//! iAP2-over-AirPlay session forwarding car GPS and vehicle speed (from Android Auto
//! SensorSourceService) to the iPhone as standard NMEA 0183 LocationInformation sentences.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use catplay_csm::{
    decoder::{CsmFlag, CsmPacketBox, CsmPacketId},
    msg::{
        DeviceTimeUpdate, EngineTypes, IdentificationInformation, LocationInformation,
        LocationInformationComponent, NowPlayingUpdate, PlaybackStatus, PowerProvidingCapability,
        StartLocationInformation, StartNowPlayingUpdates, StartNowPlayingUpdatesMediaItemAttributes,
        StartNowPlayingUpdatesPlaybackAttributes, StopLocationInformation, StopNowPlayingUpdates,
        VehicleInformationComponent, WirelessCarPlayTransportComponent,
    },
};
use catplay_iap2_client::{
    CsmClientHandleRef, CsmSession, CsmSessionResult, tokio::ClientSessionHelper,
};
use catplay_mfi::MfiDeficeRef;

const FLAG_ACC: u32 = 1 << 0;
const FLAG_ALT: u32 = 1 << 1;
const FLAG_SPEED: u32 = 1 << 2;
const FLAG_BEARING: u32 = 1 << 3;

/// Fallback UTC timestamp (2026-01-01T00:00:00Z) used only if neither the Pi clock
/// nor iPhone DeviceTimeUpdate has provided a post-2020 epoch yet.
const FALLBACK_EPOCH: u64 = 1_767_225_600;
const MIN_VALID_EPOCH: u64 = 1_577_836_800; // 2020-01-01
const MAX_VALID_EPOCH: u64 = 4_102_444_800; // 2100-01-01

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarLocation {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub acc_e3: Option<u32>,
    pub alt_e2: Option<i32>,
    pub speed_e3: Option<i32>,
    pub bearing_e6: Option<i32>,
}

impl CarLocation {
    /// Parse the 29-byte `0x27` location datagram emitted by `aa-phone`.
    pub fn from_packet(packet: &[u8]) -> Option<Self> {
        if packet.len() != 29 || packet[0] != 0x27 {
            return None;
        }
        let le_i32 = |off: usize| Some(i32::from_le_bytes(packet[off..off + 4].try_into().ok()?));
        let le_u32 = |off: usize| Some(u32::from_le_bytes(packet[off..off + 4].try_into().ok()?));
        let lat_e7 = le_i32(1)?;
        let lon_e7 = le_i32(5)?;
        if (lat_e7 == 0 && lon_e7 == 0)
            || !(-900_000_000..=900_000_000).contains(&lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&lon_e7)
        {
            return None;
        }
        let acc_raw = le_u32(9)?;
        let alt_raw = le_i32(13)?;
        let spd_raw = le_i32(17)?;
        let brg_raw = le_i32(21)?;
        let flags = le_u32(25)?;
        Some(Self {
            lat_e7,
            lon_e7,
            acc_e3: (flags & FLAG_ACC != 0).then_some(acc_raw),
            alt_e2: (flags & FLAG_ALT != 0).then_some(alt_raw),
            speed_e3: (flags & FLAG_SPEED != 0 && spd_raw >= 0).then_some(spd_raw),
            bearing_e6: (flags & FLAG_BEARING != 0 && brg_raw >= 0).then_some(brg_raw),
        })
    }
}

/// Parse the 6-byte `0x28` vehicle speed datagram emitted by `aa-phone` (m/s).
pub fn parse_speed_packet(packet: &[u8]) -> Option<f64> {
    if packet.len() != 6 || packet[0] != 0x28 {
        return None;
    }
    let speed_e3 = i32::from_le_bytes(packet[1..5].try_into().ok()?);
    if speed_e3 < 0 || speed_e3 > 150_000 {
        return None;
    }
    Some(f64::from(speed_e3) / 1000.0)
}

fn nmea_checksum(body: &str) -> u8 {
    body.bytes().fold(0u8, |acc, b| acc ^ b)
}

fn nmea_frame(body: &str) -> String {
    format!("${body}*{:02X}\r\n", nmea_checksum(body))
}

/// Convert UNIX timestamp (seconds) into `(year, month, day, hour, minute, second)` in UTC.
fn utc_ymdhms(unix_secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let rem = (unix_secs % 86_400) as u32;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    // Howard Hinnant civil-from-days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + i64::from(month <= 2);
    (year, month, day, hour, minute, second)
}

fn format_lat_dm(lat_e7: i32) -> (String, char) {
    let hemi = if lat_e7 < 0 { 'S' } else { 'N' };
    let abs_deg = (lat_e7 as i64).unsigned_abs() as f64 / 1e7;
    let deg = abs_deg.floor() as u32;
    let mins = (abs_deg - f64::from(deg)) * 60.0;
    (format!("{deg:02}{mins:07.4}"), hemi)
}

fn format_lon_dm(lon_e7: i32) -> (String, char) {
    let hemi = if lon_e7 < 0 { 'W' } else { 'E' };
    let abs_deg = (lon_e7 as i64).unsigned_abs() as f64 / 1e7;
    let deg = abs_deg.floor() as u32;
    let mins = (abs_deg - f64::from(deg)) * 60.0;
    (format!("{deg:03}{mins:07.4}"), hemi)
}

/// Format an NMEA 0183 `$GPGGA` fix sentence.
pub fn format_gpgga(loc: &CarLocation, unix_secs: u64) -> String {
    let (_, _, _, hh, mm, ss) = utc_ymdhms(unix_secs);
    let (lat, lat_h) = format_lat_dm(loc.lat_e7);
    let (lon, lon_h) = format_lon_dm(loc.lon_e7);
    let hdop = loc
        .acc_e3
        .filter(|&a| a > 0)
        .map(|a| (f64::from(a) / 5000.0).clamp(0.8, 20.0))
        .unwrap_or(1.0);
    let alt = loc
        .alt_e2
        .map(|a| format!("{:.1}", f64::from(a) / 100.0))
        .unwrap_or_else(|| "0.0".to_string());
    let body = format!(
        "GPGGA,{hh:02}{mm:02}{ss:02}.00,{lat},{lat_h},{lon},{lon_h},1,08,{hdop:.1},{alt},M,0.0,M,,"
    );
    nmea_frame(&body)
}

/// Format an NMEA 0183 `$GPRMC` transit sentence.
pub fn format_gprmc(loc: &CarLocation, fallback_speed_mps: Option<f64>, unix_secs: u64) -> String {
    let (year, month, day, hh, mm, ss) = utc_ymdhms(unix_secs);
    let (lat, lat_h) = format_lat_dm(loc.lat_e7);
    let (lon, lon_h) = format_lon_dm(loc.lon_e7);
    let speed_mps = loc
        .speed_e3
        .map(|s| f64::from(s) / 1000.0)
        .or(fallback_speed_mps)
        .unwrap_or(0.0)
        .max(0.0);
    let knots = speed_mps * 1.943_844;
    let course = loc
        .bearing_e6
        .map(|b| (f64::from(b) / 1e6).rem_euclid(360.0))
        .unwrap_or(0.0);
    let yy = year.rem_euclid(100);
    let body = format!(
        "GPRMC,{hh:02}{mm:02}{ss:02}.00,A,{lat},{lat_h},{lon},{lon_h},{knots:05.1},{course:05.1},{day:02}{month:02}{yy:02},,,A"
    );
    nmea_frame(&body)
}

/// Format an Apple MFi `$PASCD` vehicle speed NMEA sentence.
pub fn format_pascd(speed_mps: f64, uptime_secs: f64) -> String {
    let ts = uptime_secs.rem_euclid(100_000.0);
    let speed = speed_mps.max(0.0);
    let gear = if speed > 0.05 { 'D' } else { 'N' };
    let body = format!("PASCD,{ts:05.3},C,{gear},0,1,0.00,{speed:05.3}");
    nmea_frame(&body)
}

struct GpsHubInner {
    handle: Option<CsmClientHandleRef>,
    want_gpgga: bool,
    want_gprmc: bool,
    want_pascd: bool,
    last_loc: Option<(CarLocation, Instant)>,
    last_speed: Option<(f64, Instant)>,
    iphone_clock: Option<(u64, Instant)>,
    started: Instant,
    last_pascd_tx: Option<Instant>,
    tx_count: u64,
}

#[derive(Clone)]
pub struct GpsHub {
    inner: Arc<Mutex<GpsHubInner>>,
}

impl Default for GpsHub {
    fn default() -> Self {
        Self::new()
    }
}

impl GpsHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(GpsHubInner {
                handle: None,
                want_gpgga: false,
                want_gprmc: false,
                want_pascd: false,
                last_loc: None,
                last_speed: None,
                iphone_clock: None,
                started: Instant::now(),
                last_pascd_tx: None,
                tx_count: 0,
            })),
        }
    }

    fn current_utc_secs(inner: &GpsHubInner) -> u64 {
        if let Some((epoch, at)) = inner.iphone_clock {
            return epoch.saturating_add(at.elapsed().as_secs());
        }
        if let Ok(dur) = SystemTime::now().duration_since(UNIX_EPOCH) {
            let secs = dur.as_secs();
            if (MIN_VALID_EPOCH..MAX_VALID_EPOCH).contains(&secs) {
                return secs;
            }
        }
        FALLBACK_EPOCH.saturating_add(inner.started.elapsed().as_secs())
    }

    fn build_location_packet(inner: &mut GpsHubInner, loc: &CarLocation) -> Option<LocationInformation> {
        let mut sentences = Vec::with_capacity(3);
        let utc = Self::current_utc_secs(inner);
        let fallback_speed = inner
            .last_speed
            .filter(|(_, at)| at.elapsed() <= Duration::from_secs(5))
            .map(|(s, _)| s);
        if inner.want_gpgga {
            sentences.push(format_gpgga(loc, utc));
        }
        if inner.want_gprmc {
            sentences.push(format_gprmc(loc, fallback_speed, utc));
        }
        if inner.want_pascd
            && let Some(spd) = loc.speed_e3.map(|s| f64::from(s) / 1000.0).or(fallback_speed)
        {
            sentences.push(format_pascd(spd, inner.started.elapsed().as_secs_f64()));
            inner.last_pascd_tx = Some(Instant::now());
        }
        if sentences.is_empty() {
            return None;
        }
        Some(LocationInformation {
            nmea_sentence: sentences,
        })
    }

    pub fn on_location(&self, loc: CarLocation) {
        let (handle, packet, count) = {
            let mut inner = self.inner.lock().unwrap();
            inner.last_loc = Some((loc, Instant::now()));
            let Some(handle) = inner.handle.clone() else {
                return;
            };
            let Some(packet) = Self::build_location_packet(&mut inner, &loc) else {
                return;
            };
            let count = inner.tx_count;
            inner.tx_count += 1;
            (handle, packet, count)
        };
        if let Err(err) = handle.send(&packet) {
            log::warn!("CP_GPS_TX failed: {err:?}");
        } else if count.is_multiple_of(60) {
            eprintln!(
                "CP_GPS_TX n={} sentences={} lat_e7={} lon_e7={}",
                count + 1,
                packet.nmea_sentence.len(),
                loc.lat_e7,
                loc.lon_e7
            );
        }
    }

    pub fn on_speed(&self, speed_mps: f64) {
        let (handle, packet) = {
            let mut inner = self.inner.lock().unwrap();
            inner.last_speed = Some((speed_mps, Instant::now()));
            if !inner.want_pascd {
                return;
            }
            if inner
                .last_pascd_tx
                .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
            {
                return;
            }
            let Some(handle) = inner.handle.clone() else {
                return;
            };
            inner.last_pascd_tx = Some(Instant::now());
            let sentence = format_pascd(speed_mps, inner.started.elapsed().as_secs_f64());
            (
                handle,
                LocationInformation {
                    nmea_sentence: vec![sentence],
                },
            )
        };
        if let Err(err) = handle.send(&packet) {
            log::debug!("CP_GPS_PASCD_TX failed: {err:?}");
        }
    }

    pub fn on_device_time(&self, epoch_secs: u64) {
        if (MIN_VALID_EPOCH..MAX_VALID_EPOCH).contains(&epoch_secs) {
            self.inner.lock().unwrap().iphone_clock = Some((epoch_secs, Instant::now()));
        }
    }

    pub fn on_start_location(&self, req: &StartLocationInformation, handle: CsmClientHandleRef) {
        let initial = {
            let mut inner = self.inner.lock().unwrap();
            inner.want_gpgga = req.global_positioning_system_fix_data == CsmFlag::Yes;
            inner.want_gprmc =
                req.recommended_minimum_specific_gps_transit_data == CsmFlag::Yes;
            inner.want_pascd = req.vehicle_speed_data == CsmFlag::Yes;
            inner.handle = Some(handle.clone());
            eprintln!(
                "CP_GPS_SUBSCRIBED gpgga={} gprmc={} pascd={}",
                inner.want_gpgga, inner.want_gprmc, inner.want_pascd
            );
            inner
                .last_loc
                .filter(|(_, at)| at.elapsed() <= Duration::from_secs(5))
                .map(|(loc, _)| loc)
                .and_then(|loc| Self::build_location_packet(&mut inner, &loc))
        };
        if let Some(packet) = initial {
            let _ = handle.send(&packet);
        }
    }

    pub fn on_stop_location(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.want_gpgga = false;
        inner.want_gprmc = false;
        inner.want_pascd = false;
        eprintln!("CP_GPS_UNSUBSCRIBED");
    }

    pub fn clear_handle(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.handle = None;
        inner.want_gpgga = false;
        inner.want_gprmc = false;
        inner.want_pascd = false;
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Encode NowPlaying fields into the compact `0x30` control datagram read by `aa-phone`.
pub fn encode_now_playing_packet(
    state: u8,
    duration_secs: u16,
    title: &str,
    artist: &str,
    album: &str,
    app: &str,
) -> Vec<u8> {
    let fields = [
        truncate_utf8(title, 96).as_bytes(),
        truncate_utf8(artist, 96).as_bytes(),
        truncate_utf8(album, 96).as_bytes(),
        truncate_utf8(app, 48).as_bytes(),
    ];
    let mut out = Vec::with_capacity(8 + fields.iter().map(|f| f.len()).sum::<usize>());
    out.push(0x30);
    out.push(state);
    out.extend_from_slice(&duration_secs.to_le_bytes());
    for f in fields {
        out.push(f.len() as u8);
        out.extend_from_slice(f);
    }
    out
}

pub struct GpsIap2Session {
    mfi: Option<MfiDeficeRef>,
    id: IdentificationInformation,
    hub: GpsHub,
    socket: String,
    now_playing: Option<NowPlayingUpdate>,
    last_meta_packet: Vec<u8>,
    now_playing_started: bool,
}

impl Drop for GpsIap2Session {
    fn drop(&mut self) {
        self.hub.clear_handle();
    }
}

impl GpsIap2Session {
    pub fn new(mfi: Option<MfiDeficeRef>, display_name: String, hub: GpsHub, socket: String) -> Self {
        let id = Self::build_identification(&display_name);
        Self {
            mfi,
            id,
            hub,
            socket,
            now_playing: None,
            last_meta_packet: Vec::new(),
            now_playing_started: false,
        }
    }

    fn start_now_playing_request() -> StartNowPlayingUpdates {
        StartNowPlayingUpdates {
            media_item_attributes: Some(StartNowPlayingUpdatesMediaItemAttributes {
                media_item_persistent_identifier: CsmFlag::Yes,
                media_item_title: CsmFlag::Yes,
                media_item_artist: CsmFlag::Yes,
                media_item_album_title: CsmFlag::Yes,
                media_item_playback_duration_in_milliseconds: CsmFlag::Yes,
                ..StartNowPlayingUpdatesMediaItemAttributes::default()
            }),
            playback_attributes: Some(StartNowPlayingUpdatesPlaybackAttributes {
                playback_status: CsmFlag::Yes,
                playback_elapsed_time_in_milliseconds: CsmFlag::Yes,
                playback_app_name: CsmFlag::Yes,
                playback_app_bundle_id: CsmFlag::Yes,
                ..StartNowPlayingUpdatesPlaybackAttributes::default()
            }),
            playback_queue_list_content_transfer_info_request: None,
        }
    }

    fn forward_now_playing(&mut self, update: NowPlayingUpdate) {
        match &mut self.now_playing {
            Some(current) => current.deep_merge(update),
            None => self.now_playing = Some(update),
        }
        let Some(np) = &self.now_playing else {
            return;
        };
        let item = np.media_item_attributes.as_ref();
        let pb = np.playback_attributes.as_ref();
        let title = item.and_then(|m| m.media_item_title.as_deref()).unwrap_or("");
        let artist = item.and_then(|m| m.media_item_artist.as_deref()).unwrap_or("");
        let album = item.and_then(|m| m.media_item_album_title.as_deref()).unwrap_or("");
        let app = pb.and_then(|p| p.playback_app_name.as_deref()).unwrap_or("CarPlay");
        let duration_secs = item
            .and_then(|m| m.media_item_playback_duration_in_milliseconds)
            .map(|ms| (ms / 1000).min(u32::from(u16::MAX)) as u16)
            .unwrap_or(0);
        let state = match pb.and_then(|p| p.playback_status) {
            Some(PlaybackStatus::Playing) => 2,
            Some(PlaybackStatus::Paused) => 3,
            Some(PlaybackStatus::Stopped) => 1,
            _ => 0,
        };
        if title.is_empty() && artist.is_empty() && album.is_empty() {
            return;
        }
        let pkt = encode_now_playing_packet(state, duration_secs, title, artist, album, app);
        if pkt == self.last_meta_packet {
            return;
        }
        self.last_meta_packet = pkt.clone();
        if let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() {
            let _ = sock.set_nonblocking(true);
            let _ = sock.send_to(&pkt, format!("{}.control", self.socket));
        }
        eprintln!("CP_NOW_PLAYING state={state} dur={duration_secs}s title={title:?} artist={artist:?}");
    }

    pub fn build_identification(display_name: &str) -> IdentificationInformation {
        let mut id = IdentificationInformation {
            name: display_name.to_string(),
            model_identifier: "CarFluxoid".into(),
            manufacturer: "CarFluxoid".into(),
            serial_number: "CFX-0001".into(),
            firmware_version: "1.0".into(),
            hardware_version: "1.0".into(),
            ..IdentificationInformation::default()
        };
        id.messages_sent_by_accessory = IdentificationInformation::pack_ids(&[
            LocationInformation::PACKET_ID,
            StartNowPlayingUpdates::PACKET_ID,
            StopNowPlayingUpdates::PACKET_ID,
        ]);
        id.messages_received_from_device = IdentificationInformation::pack_ids(&[
            StartLocationInformation::PACKET_ID,
            StopLocationInformation::PACKET_ID,
            DeviceTimeUpdate::PACKET_ID,
            NowPlayingUpdate::PACKET_ID,
        ]);
        id.power_providing_capability = PowerProvidingCapability::None;
        id.maximum_current_drawn_from_device = 0;
        id.supported_language = vec!["en".into()];
        id.current_language = "en".into();
        id.vehicle_information_component = Some(VehicleInformationComponent {
            identifier: 1,
            name: "VEH_INFO_USE".into(),
            display_name: display_name.to_string(),
            engine_type: vec![EngineTypes::Gasoline],
            ..VehicleInformationComponent::default()
        });
        id.location_information_component = Some(LocationInformationComponent {
            identifier: 4001,
            name: "LOC_USE".into(),
            global_positioning_system_fix_data: CsmFlag::Yes,
            recommended_minimum_specific_gps_transit_data: CsmFlag::Yes,
            gps_satellites_in_view: CsmFlag::No,
            vehicle_speed_data: CsmFlag::Yes,
            vehicle_gyro_data: CsmFlag::No,
            vehicle_accelerometer_data: CsmFlag::No,
            vehicle_heading_data: CsmFlag::No,
        });
        id.wireless_car_play_transport_component = vec![WirelessCarPlayTransportComponent {
            transport_component_identifier: 2,
            transport_component_name: "IAP2-Wireless".into(),
            transport_supports_iap2_connection: CsmFlag::Yes,
            transport_supports_car_play: CsmFlag::Yes,
            ..WirelessCarPlayTransportComponent::default()
        }];
        id
    }
}

#[async_trait]
impl CsmSession for GpsIap2Session {
    async fn start(&mut self, _handle: CsmClientHandleRef) -> CsmSessionResult<()> {
        eprintln!("CP_IAP2_STARTED");
        Ok(())
    }

    async fn respond(
        &mut self,
        packet: CsmPacketBox,
        handle: CsmClientHandleRef,
    ) -> CsmSessionResult<()> {
        if let Some(time) = packet.cast::<DeviceTimeUpdate>() {
            self.hub.on_device_time(time.seconds_since_reference_date);
        }
        if let Some(np) = packet.cast::<NowPlayingUpdate>() {
            self.forward_now_playing(np.clone());
            return Ok(());
        }
        if let Some(start) = packet.cast::<StartLocationInformation>() {
            self.hub.on_start_location(start, handle.clone());
            return Ok(());
        }
        if packet.cast::<StopLocationInformation>().is_some() {
            self.hub.on_stop_location();
            return Ok(());
        }
        if ClientSessionHelper::handle_auth(self.mfi.clone(), &packet, handle.clone()).await? {
            return Ok(());
        }
        let mut accepted = false;
        if ClientSessionHelper::handle_id(&packet, handle.clone(), &mut accepted, &self.id).await? {
            if accepted {
                eprintln!("CP_IAP2_IDENTIFIED gps=enabled now_playing=enabled");
                if !self.now_playing_started {
                    self.now_playing_started = true;
                    match handle.send(&Self::start_now_playing_request()) {
                        Ok(()) => eprintln!("CP_NOW_PLAYING_SUBSCRIBED"),
                        Err(err) => log::warn!("CP_NOW_PLAYING_SUBSCRIBE failed: {err:?}"),
                    }
                }
            }
            return Ok(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_nmea_checksum(sentence: &str) {
        assert!(sentence.starts_with('$'), "missing $: {sentence:?}");
        assert!(sentence.ends_with("\r\n"), "missing CRLF: {sentence:?}");
        let trimmed = &sentence[1..sentence.len() - 2];
        let (body, hex) = trimmed.split_once('*').expect("missing *");
        let expected = u8::from_str_radix(hex, 16).expect("invalid hex checksum");
        assert_eq!(nmea_checksum(body), expected, "bad checksum for {sentence:?}");
    }

    #[test]
    fn formats_valid_gpgga_gprmc_and_pascd_sentences() {
        // Paris (48.8566 N, 2.3522 E), alt 35.5 m, speed 10.0 m/s, bearing 180.5 deg
        // 2026-11-21 12:34:56 UTC = 1795264496
        let loc = CarLocation {
            lat_e7: 488_566_000,
            lon_e7: 23_522_000,
            acc_e3: Some(5_000),
            alt_e2: Some(3_550),
            speed_e3: Some(10_000),
            bearing_e6: Some(180_500_000),
        };
        let ts = 1_795_264_496u64;
        assert_eq!(utc_ymdhms(ts), (2026, 11, 21, 12, 34, 56));

        let gga = format_gpgga(&loc, ts);
        verify_nmea_checksum(&gga);
        assert!(gga.starts_with("$GPGGA,123456.00,4851.3960,N,00221.1320,E,1,08,1.0,35.5,M,0.0,M,,*"));

        let rmc = format_gprmc(&loc, None, ts);
        verify_nmea_checksum(&rmc);
        assert!(rmc.starts_with("$GPRMC,123456.00,A,4851.3960,N,00221.1320,E,019.4,180.5,211126,,,A*"));

        let pascd = format_pascd(10.0, 12.345);
        verify_nmea_checksum(&pascd);
        assert!(pascd.starts_with("$PASCD,12.345,C,D,0,1,0.00,10.000*"));
    }

    #[test]
    fn handles_southern_and_western_hemispheres() {
        // Buenos Aires (-34.6037 S, -58.3816 W)
        let loc = CarLocation {
            lat_e7: -346_037_000,
            lon_e7: -583_816_000,
            acc_e3: None,
            alt_e2: None,
            speed_e3: None,
            bearing_e6: None,
        };
        let gga = format_gpgga(&loc, FALLBACK_EPOCH);
        verify_nmea_checksum(&gga);
        assert!(gga.contains(",3436.2220,S,05822.8960,W,1,08,1.0,0.0,M,0.0,M,,*"));
    }

    #[test]
    fn decodes_and_validates_binary_location_and_speed_packets() {
        let mut pkt = [0u8; 29];
        pkt[0] = 0x27;
        pkt[1..5].copy_from_slice(&488_566_000i32.to_le_bytes());
        pkt[5..9].copy_from_slice(&(-23_522_000i32).to_le_bytes());
        pkt[9..13].copy_from_slice(&4_500u32.to_le_bytes());
        pkt[13..17].copy_from_slice(&3_550i32.to_le_bytes());
        pkt[17..21].copy_from_slice(&12_500i32.to_le_bytes());
        pkt[21..25].copy_from_slice(&90_000_000i32.to_le_bytes());
        pkt[25..29].copy_from_slice(&(FLAG_ACC | FLAG_ALT | FLAG_SPEED | FLAG_BEARING).to_le_bytes());
        let loc = CarLocation::from_packet(&pkt).expect("valid location");
        assert_eq!(loc.lat_e7, 488_566_000);
        assert_eq!(loc.lon_e7, -23_522_000);
        assert_eq!(loc.acc_e3, Some(4_500));
        assert_eq!(loc.alt_e2, Some(3_550));
        assert_eq!(loc.speed_e3, Some(12_500));
        assert_eq!(loc.bearing_e6, Some(90_000_000));

        // Reject Null Island (0, 0)
        let mut zero = [0u8; 29];
        zero[0] = 0x27;
        assert!(CarLocation::from_packet(&zero).is_none());

        // Speed packet
        let mut spd = [0u8; 6];
        spd[0] = 0x28;
        spd[1..5].copy_from_slice(&13_889i32.to_le_bytes());
        assert!((parse_speed_packet(&spd).unwrap() - 13.889).abs() < 1e-6);
    }

    #[test]
    fn builds_wireless_iap2_identification_with_location_component() {
        let id = GpsIap2Session::build_identification("CarFluxoid");
        let loc = id.location_information_component.expect("location component");
        assert_eq!(loc.global_positioning_system_fix_data, CsmFlag::Yes);
        assert_eq!(loc.recommended_minimum_specific_gps_transit_data, CsmFlag::Yes);
        assert_eq!(loc.vehicle_speed_data, CsmFlag::Yes);
        assert_eq!(id.wireless_car_play_transport_component.len(), 1);
    }

    #[test]
    fn encodes_now_playing_metadata_with_utf8_safe_truncation() {
        let long_title = "é".repeat(60); // 120 bytes UTF-8
        let pkt = encode_now_playing_packet(2, 245, &long_title, "Daft Punk", "Discovery", "Spotify");
        assert_eq!(pkt[0], 0x30);
        assert_eq!(pkt[1], 2);
        assert_eq!(u16::from_le_bytes([pkt[2], pkt[3]]), 245);
        let tlen = pkt[4] as usize;
        assert_eq!(tlen, 96);
        assert!(std::str::from_utf8(&pkt[5..5 + tlen]).is_ok());
        let a_off = 5 + tlen;
        assert_eq!(pkt[a_off] as usize, 9);
        assert_eq!(&pkt[a_off + 1..a_off + 10], b"Daft Punk");
    }
}

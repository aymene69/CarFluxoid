# Display configuration

The local page exposes independent controls:

- Interface scale: 75, 85, 100, 115, 125 or 150 percent. It changes the physical display dimensions announced to the iPhone, preserving pixel resolution and touch coordinates. The former width of 160 mm is the 100% reference; larger requested scales announce smaller physical dimensions. iOS chooses the final layout, so percentages are indicative rather than an exact pixel zoom.
- Maximum resolution: automatic, 800×480, 1280×720 or 1920×1080. Only configurations actually advertised by the Android Auto receiver can be chosen. If none fits the cap, the smallest available configuration is used. Both sides use the selected resolution and its real AA configuration index; no video resizing is performed.
- Maximum frame rate: 10, 15, 20 or 30 fps, requested from the iPhone.
- Assisted/direct touch capability, sidebar side, day/night appearance and OEM return app identity/visibility.

Scale and CarPlay options apply on reconnection. Resolution is selected at Android Auto startup and needs a dongle restart. H.264 bitrate control is not implemented. Changing interface scale alone does not guarantee lower load or better audio.

The code announces CarFluxoid for the accessory name, model and manufacturer. An old CarPlay profile name saved on the iPhone may remain until that old vehicle entry is forgotten once in Settings > General > CarPlay.

## DHU resolution and interface scale

Use `resolution = 800x480`, `1280x720` or `1920x1080` for the documented DHU landscape formats. A request such as `400x240` is rejected by DHU and falls back to 800x480. To change icon/text sizing at the same pixel resolution, use the CarFluxoid interface-scale setting instead. This does not change the video format advertised by DHU.

Reference: https://developer.android.com/training/cars/testing/dhu

IMAGE    ?= carfluxoid-pi-build:20261122
OS_IMAGE ?= carfluxoid-os-build:20261122
HOST     ?= root@192.168.50.2
DOCKER_RUN = docker run --rm --platform linux/arm64 \
	-v "$(CURDIR):/input" \
	-v carfluxoid-pi-build:/build \
	-v carfluxoid-cargo-registry:/opt/cargo/registry \
	-e CARGO_HOME=/opt/cargo \
	-e CARGO_TARGET_DIR=/build/pi/target \
	-e OPENSSL_NO_VENDOR=1 \
	$(IMAGE)

.PHONY: help prepare image build test tls os-image os deploy os-update backup restore logs

help:
	@echo "CarFluxoid — Raspberry Pi Zero 2 W Wireless CarPlay-to-Android-Auto Bridge"
	@echo ""
	@echo "Targets:"
	@echo "  make prepare                  Initialize CatPlay submodule and apply patches/catplay.patch"
	@echo "  make image                    Build the self-contained ARM64 Docker build image"
	@echo "  make build                    Build ARM64 binaries, update-bundle/ and carfluxoid-update.tar.gz"
	@echo "  make test                     Run Rust, C, web, MTP, and dashboard test suites"
	@echo "  make tls                      Download & verify reference Android Auto TLS certificate/key"
	@echo "  make os-image                 Build the CarFluxoidOS Buildroot Docker image"
	@echo "  make os                       Build CarFluxoidOS SD image (.img.xz) + A/B update (.tar)"
	@echo "  make deploy    [HOST=u@ip]    Fast app update over SSH (never touches /data)"
	@echo "  make os-update [HOST=u@ip]    Atomic A/B OS update over SSH (never touches /data)"
	@echo "  make backup    [HOST=u@ip]    Backup /data (certs, pairings, settings) to local/os/data-backup.tar.gz"
	@echo "  make restore   [HOST=u@ip]    Restore local/os/data-backup.tar.gz onto /data and reboot"
	@echo "  make logs      [HOST=u@ip]    Fetch the latest vehicle diagnostic log to local/raspberrypi/"

prepare:
	./scripts/prepare-catplay.sh

image:
	docker build --platform linux/arm64 -t $(IMAGE) -f platforms/raspberrypi/Dockerfile .

build: prepare
	$(DOCKER_RUN) sh /input/platforms/raspberrypi/build.sh

test:
	$(DOCKER_RUN) sh /input/platforms/raspberrypi/test.sh

tls:
	python3 scripts/fetch-tls.py

os-image:
	docker build --platform linux/arm64 -t $(OS_IMAGE) -f os/Dockerfile os/

os:
	docker run --rm --platform linux/arm64 \
		-v "$(CURDIR):/input" \
		-v carfluxoid-br:/br \
		$(OS_IMAGE) sh /input/os/build.sh

deploy:
	scp -r local/raspberrypi/update-bundle $(HOST):/tmp/carfluxoid-update
	ssh -t $(HOST) 'if [ "$$(id -u)" -eq 0 ]; then /tmp/carfluxoid-update/install-dashboard.sh; else sudo /tmp/carfluxoid-update/install-dashboard.sh; fi && rm -rf /tmp/carfluxoid-update'

os-update:
	@test -s local/os/carfluxoid-os-update.tar || { echo "Missing local/os/carfluxoid-os-update.tar — run 'make os' first" >&2; exit 1; }
	ssh $(HOST) '/usr/sbin/carfluxoid-os-update' < local/os/carfluxoid-os-update.tar

backup:
	@mkdir -p local/os
	ssh $(HOST) 'tar -C /data -czf - identity pairing bluetooth settings radio.env dropbear tls 2>/dev/null' > local/os/data-backup.tar.gz
	@ls -lh local/os/data-backup.tar.gz

restore:
	@test -s local/os/data-backup.tar.gz || { echo "Missing local/os/data-backup.tar.gz — run 'make backup' first" >&2; exit 1; }
	ssh $(HOST) 'tar -C /data -xzf - && sync && (sleep 1; reboot) >/dev/null 2>&1 &' < local/os/data-backup.tar.gz

logs:
	ssh -t $(HOST) 'if [ "$$(id -u)" -eq 0 ]; then cp /var/lib/carfluxoid/car-diag.prev.log /tmp/car-diag.log && chmod 644 /tmp/car-diag.log; else sudo cp /var/lib/carfluxoid/car-diag.prev.log /tmp/car-diag.log && sudo chmod 644 /tmp/car-diag.log; fi'
	scp $(HOST):/tmp/car-diag.log local/raspberrypi/car-diag-last.log

IMAGE ?= carfluxoid-pi-build:20261122
HOST  ?= aymene@192.168.50.2
DOCKER_RUN = docker run --rm --platform linux/arm64 \
	-v "$(CURDIR):/input" \
	-v carfluxoid-pi-build:/build \
	-v carfluxoid-cargo-registry:/opt/cargo/registry \
	-e CARGO_HOME=/opt/cargo \
	-e CARGO_TARGET_DIR=/build/pi/target \
	-e OPENSSL_NO_VENDOR=1 \
	$(IMAGE)

.PHONY: help prepare image build test tls deploy logs

help:
	@echo "CarFluxoid — Raspberry Pi Zero 2 W Wireless CarPlay-to-Android-Auto Bridge"
	@echo ""
	@echo "Targets:"
	@echo "  make prepare               Initialize CatPlay submodule and apply patches/catplay.patch"
	@echo "  make image                 Build the self-contained ARM64 Docker build image"
	@echo "  make build                 Build ARM64 binaries, update-bundle/ and carfluxoid-update.tar.gz"
	@echo "  make test                  Run Rust, C, web, MTP, and dashboard test suites"
	@echo "  make tls                   Download & verify reference Android Auto TLS certificate/key"
	@echo "  make deploy HOST=user@ip   Deploy update-bundle to a Raspberry Pi over SSH"
	@echo "  make logs   HOST=user@ip   Fetch the latest vehicle diagnostic log to local/raspberrypi/"

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

deploy:
	scp -r local/raspberrypi/update-bundle $(HOST):/tmp/carfluxoid-update
	ssh -t $(HOST) 'sudo /tmp/carfluxoid-update/install-dashboard.sh && rm -rf /tmp/carfluxoid-update'

logs:
	ssh -t $(HOST) 'sudo cp /var/lib/carfluxoid/car-diag.prev.log /tmp/car-diag.log && sudo chmod 644 /tmp/car-diag.log'
	scp $(HOST):/tmp/car-diag.log local/raspberrypi/car-diag-last.log

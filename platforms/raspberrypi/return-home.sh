#!/bin/sh
set -eu
systemctl stop carfluxoid-radio.service
nmcli connection up uuid "$1"

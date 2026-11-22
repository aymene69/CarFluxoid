#!/usr/bin/env python3
"""Download and verify the reference Android Auto TLS certificate/key pair into local/tls/."""
import hashlib
import os
import subprocess
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE_URL = (
    "https://raw.githubusercontent.com/iscle/android-auto-certificates/"
    "8dc25fe2bd2c57e0c6f48488bb1095ecc0c11e31/android-auto/latest/"
)
FILES = [
    ("test-cert.pem", "certificate.pem", "43c05285265d83a9e1ae74da7359f07c081c711354a2c397292419169d8c360f"),
    ("test-key.pem", "private-key.pem", "e30eade4b8f3384b739400a988be494d639e92264e8f60414f17c4c931f333e2"),
]


def main() -> None:
    folder = ROOT / "local" / "tls"
    folder.mkdir(parents=True, exist_ok=True)
    os.chmod(folder, 0o700)
    for local_name, remote_name, expected_sha in FILES:
        dest = folder / local_name
        if not dest.exists():
            with urllib.request.urlopen(BASE_URL + remote_name, timeout=60) as resp:
                data = resp.read()
            if hashlib.sha256(data).hexdigest() != expected_sha:
                raise RuntimeError(f"Checksum mismatch for {remote_name}")
            fd = os.open(dest, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(data)
    cert_pub = subprocess.check_output(["openssl", "x509", "-in", str(folder / "test-cert.pem"), "-pubkey", "-noout"])
    key_pub = subprocess.check_output(["openssl", "pkey", "-in", str(folder / "test-key.pem"), "-pubout"])
    if cert_pub != key_pub:
        raise RuntimeError("TLS private key does not match certificate")
    subprocess.run(["openssl", "x509", "-checkend", "0", "-noout", "-in", str(folder / "test-cert.pem")], check=True)
    print(f"Reference Android Auto TLS pair ready in {folder}")


if __name__ == "__main__":
    main()

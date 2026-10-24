#!/usr/bin/env python3

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import urllib.request

PROJECT_ROOT = Path(__file__).resolve().parents[1]
ROOTFS = PROJECT_ROOT / "rootfs"
ARTIFACTS = PROJECT_ROOT / "artifacts"
PACKAGE_CACHE = ARTIFACTS / "packages"
SOURCE_JSON = ARTIFACTS / "rootfs-source.json"

ALPINE_RELEASE = "v3.22"
REPOSITORY = "main"
ARCHITECTURE = "riscv64"
BASE_URL = (
    f"https://dl-cdn.alpinelinux.org/alpine/{ALPINE_RELEASE}/"
    f"{REPOSITORY}/{ARCHITECTURE}"
)
APKINDEX_URL = f"{BASE_URL}/APKINDEX.tar.gz"
APKINDEX_CACHE = PACKAGE_CACHE / "APKINDEX.tar.gz"

PACKAGES = (
    {
        "name": "nginx",
        "version": "1.28.3-r7",
        "sha256": "ad42325769face2a684a81bea94e73982df4f1cd23dcb450bec89fa9968a1c92",
        "files": (
            "usr/sbin/nginx",
            "etc/nginx/mime.types",
            "var/lib/nginx/html/50x.html",
        ),
    },
    {
        "name": "musl",
        "version": "1.2.5-r13",
        "sha256": "27760752b676b78f1edca22b7d933515ea9ed3596bb7378da8d2bb81c873a3e6",
        "files": (
            "lib/ld-musl-riscv64.so.1",
            "lib/libc.musl-riscv64.so.1",
        ),
    },
    {
        "name": "libssl3",
        "version": "3.5.9-r0",
        "sha256": "528e80a3ef436e662adb66be1638454cc435c28cf4df678eb17332e99d0c2ec5",
        "files": ("usr/lib/libssl.so.3",),
    },
    {
        "name": "libcrypto3",
        "version": "3.5.9-r0",
        "sha256": "2bb9af0e10adf4620074c9315c0db57dcd8b1c0492172d6782f59bb20e122cd0",
        "files": ("usr/lib/libcrypto.so.3",),
    },
    {
        "name": "pcre2",
        "version": "10.46-r1",
        "sha256": "f7663832d8a2661b17bbd6552d11ebd523c6049d121f8fe9826946aed9c72225",
        "files": (
            "usr/lib/libpcre2-8.so.0",
            "usr/lib/libpcre2-8.so.0.14.0",
        ),
    },
    {
        "name": "zlib",
        "version": "1.3.2-r1",
        "sha256": "9dfa50a52957f9d04b90b70c9fdfb08e1adca326fc57a3552d6e41047434e02c",
        "files": (
            "usr/lib/libz.so.1",
            "usr/lib/libz.so.1.3.2",
        ),
    },
)

NGINX_CONFIG = """\
daemon off;
master_process off;
worker_processes 1;
error_log stderr notice;
pid /run/nginx.pid;
lock_file /run/nginx.lock;

events {
    worker_connections 128;
}

http {
    include /etc/nginx/mime.types;
    default_type application/octet-stream;
    access_log off;
    sendfile off;

    client_body_temp_path /var/lib/nginx/tmp/client_body;
    proxy_temp_path /var/lib/nginx/tmp/proxy;
    fastcgi_temp_path /var/lib/nginx/tmp/fastcgi;
    uwsgi_temp_path /var/lib/nginx/tmp/uwsgi;
    scgi_temp_path /var/lib/nginx/tmp/scgi;

    server {
        listen 0.0.0.0:80 default_server;
        server_name _;
        root /var/www/html;
        index index.html;

        location / {
            try_files $uri $uri/ =404;
        }

        error_page 500 502 503 504 /50x.html;
        location = /50x.html {
            root /var/lib/nginx/html;
        }
    }
}
"""

INDEX_HTML = """\
<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>RISC-V nginx</title>
</head>
<body>
    <h1>RISCV_NGINX_OK</h1>
    <p>The official Alpine nginx riscv64 binary is running.</p>
</body>
</html>
"""

EMPTY_DIRECTORIES = (
    ("run", 0o755),
    ("tmp", 0o1777),
    ("var/log/nginx", 0o755),
    ("var/lib/nginx/logs", 0o755),
    ("var/lib/nginx/tmp", 0o755),
    ("var/lib/nginx/tmp/client_body", 0o755),
    ("var/lib/nginx/tmp/proxy", 0o755),
    ("var/lib/nginx/tmp/fastcgi", 0o755),
    ("var/lib/nginx/tmp/uwsgi", 0o755),
    ("var/lib/nginx/tmp/scgi", 0o755),
    ("var/www/html", 0o755),
)


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download_if_missing(url, destination, expected_sha256=None):
    if destination.exists():
        if expected_sha256 is not None:
            actual = sha256_file(destination)
            if actual != expected_sha256:
                raise RuntimeError(
                    f"cached {destination} has SHA256 {actual}, expected {expected_sha256}"
                )
        return

    print(f"Downloading {url}")
    partial = destination.with_name(destination.name + ".part")
    try:
        request = urllib.request.Request(
            url, headers={"User-Agent": "fetch_rootfs.py/1"}
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            with partial.open("wb") as output:
                shutil.copyfileobj(response, output)
        if expected_sha256 is not None:
            actual = sha256_file(partial)
            if actual != expected_sha256:
                raise RuntimeError(
                    f"downloaded {url} has SHA256 {actual}, expected {expected_sha256}"
                )
        os.replace(partial, destination)
    finally:
        partial.unlink(missing_ok=True)


def read_apkindex(path):
    with tarfile.open(path, "r:gz") as archive:
        members = [
            member
            for member in archive.getmembers()
            if member.name == "APKINDEX" and member.isfile()
        ]
        if len(members) != 1:
            raise RuntimeError(f"{path} does not contain exactly one APKINDEX")
        extracted = archive.extractfile(members[0])
        if extracted is None:
            raise RuntimeError(f"cannot read APKINDEX in {path}")
        text = extracted.read().decode("utf-8")

    records = {}
    for block in text.strip().split("\n\n"):
        fields = {}
        for line in block.splitlines():
            key, separator, value = line.partition(":")
            if separator:
                fields[key] = value
        name = fields.get("P")
        if name:
            if name in records:
                raise RuntimeError(f"duplicate APKINDEX record for {name}")
            records[name] = fields
    return records


def destination_for(member_name):
    path = PurePosixPath(member_name)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in ("", ".", "..") for part in path.parts)
    ):
        raise RuntimeError(f"unsafe archive path: {member_name!r}")

    destination = ROOTFS.joinpath(*path.parts)
    current = ROOTFS
    for part in path.parts[:-1]:
        current = current / part
        if current.is_symlink():
            raise RuntimeError(f"archive path traverses a symlink: {member_name!r}")
    if destination.exists() or destination.is_symlink():
        raise RuntimeError(f"duplicate output path: {member_name!r}")
    return destination


def extract_member(archive, member):
    destination = destination_for(member.name)
    destination.parent.mkdir(parents=True, exist_ok=True)

    if member.isfile():
        source = archive.extractfile(member)
        if source is None:
            raise RuntimeError(f"cannot extract {member.name!r}")
        with source:
            with destination.open("xb") as output:
                shutil.copyfileobj(source, output)
        if destination.stat().st_size != member.size:
            raise RuntimeError(f"short extraction for {member.name!r}")
        destination.chmod(member.mode & 0o777)
        return

    if member.issym():
        target = PurePosixPath(member.linkname)
        if target.is_absolute() or len(target.parts) != 1 or target.parts[0] == "..":
            raise RuntimeError(
                f"unsafe symlink {member.name!r} -> {member.linkname!r}"
            )
        os.symlink(member.linkname, destination)
        return

    raise RuntimeError(f"unsupported archive member type for {member.name!r}")


def extract_package(package, apk_path):
    expected = set(package["files"])
    seen = set()
    with tarfile.open(apk_path, "r:gz") as archive:
        for member in archive:
            if member.name not in expected:
                continue
            if member.name in seen:
                raise RuntimeError(
                    f"duplicate archive member {member.name!r} in {apk_path}"
                )
            extract_member(archive, member)
            seen.add(member.name)

    missing = expected - seen
    if missing:
        raise RuntimeError(f"missing members in {apk_path}: {sorted(missing)}")


def rebuild_rootfs(package_paths):
    if ROOTFS.is_symlink():
        raise RuntimeError(f"refusing to replace symlinked rootfs: {ROOTFS}")
    if ROOTFS.exists():
        shutil.rmtree(ROOTFS)
    ROOTFS.mkdir(parents=True)

    for package in PACKAGES:
        extract_package(package, package_paths[package["name"]])

    for relative_path, mode in EMPTY_DIRECTORIES:
        directory = ROOTFS / relative_path
        directory.mkdir(parents=True, exist_ok=True)
        directory.chmod(mode)

    (ROOTFS / "etc/nginx/nginx.conf").write_bytes(NGINX_CONFIG.encode("utf-8"))
    (ROOTFS / "var/www/html/index.html").write_bytes(INDEX_HTML.encode("utf-8"))
    (ROOTFS / "etc/passwd").write_text(
        "root:x:0:0:root:/root:/bin/sh\n"
        "nginx:x:1000:1000:nginx:/var/lib/nginx:/sbin/nologin\n",
        encoding="utf-8",
    )
    (ROOTFS / "etc/group").write_text(
        "root:x:0:\nnginx:x:1000:\n", encoding="utf-8"
    )

    required_files = {
        "etc/group",
        "etc/passwd",
        "etc/nginx/mime.types",
        "etc/nginx/nginx.conf",
        "lib/ld-musl-riscv64.so.1",
        "lib/libc.musl-riscv64.so.1",
        "usr/lib/libcrypto.so.3",
        "usr/lib/libssl.so.3",
        "usr/lib/libpcre2-8.so.0",
        "usr/lib/libz.so.1",
        "usr/sbin/nginx",
        "var/lib/nginx/html/50x.html",
        "var/www/html/index.html",
    }
    missing = sorted(path for path in required_files if not (ROOTFS / path).exists())
    if missing:
        raise RuntimeError(f"rootfs is missing required files: {missing}")


def write_source_metadata(records, package_paths):
    packages = []
    for package in PACKAGES:
        name = package["name"]
        filename = f"{name}-{package['version']}.apk"
        packages.append(
            {
                "name": name,
                "version": package["version"],
                "filename": filename,
                "url": f"{BASE_URL}/{filename}",
                "size": package_paths[name].stat().st_size,
                "sha256": package["sha256"],
                "apkindex_dependencies": records[name].get("D", "").split(),
            }
        )

    nginx_path = ROOTFS / "usr/sbin/nginx"
    metadata = {
        "alpine_release": ALPINE_RELEASE,
        "repository": REPOSITORY,
        "architecture": ARCHITECTURE,
        "apkindex": {
            "url": APKINDEX_URL,
            "sha256": sha256_file(APKINDEX_CACHE),
        },
        "verification": {
            "package_sha256": "verified against constants pinned in scripts/fetch_rootfs.py",
            "package_versions_and_sizes": "verified against the downloaded Alpine APKINDEX",
            "apk_signatures_verified": False,
            "signature_note": "APK signature files are not cryptographically verified by this script.",
        },
        "packages": packages,
        "nginx": {
            "path": "/usr/sbin/nginx",
            "source_package": "nginx-1.28.3-r7.apk",
            "source_package_sha256": PACKAGES[0]["sha256"],
            "source_path": "usr/sbin/nginx",
            "size": nginx_path.stat().st_size,
            "sha256": sha256_file(nginx_path),
            "elf_bytes_modified": False,
        },
        "locally_generated_files": (
            "/etc/group",
            "/etc/nginx/nginx.conf",
            "/etc/passwd",
            "/var/www/html/index.html",
        ),
    }
    SOURCE_JSON.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main():
    PACKAGE_CACHE.mkdir(parents=True, exist_ok=True)
    ARTIFACTS.mkdir(parents=True, exist_ok=True)

    download_if_missing(APKINDEX_URL, APKINDEX_CACHE)
    records = read_apkindex(APKINDEX_CACHE)
    package_paths = {}

    for package in PACKAGES:
        name = package["name"]
        record = records.get(name)
        if record is None:
            raise RuntimeError(f"{name} is absent from {APKINDEX_URL}")
        if record.get("V") != package["version"]:
            raise RuntimeError(
                f"APKINDEX has {name} {record.get('V')}, expected {package['version']}"
            )

        filename = f"{name}-{package['version']}.apk"
        apk_path = PACKAGE_CACHE / filename
        download_if_missing(
            f"{BASE_URL}/{filename}", apk_path, package["sha256"]
        )
        actual_size = apk_path.stat().st_size
        indexed_size = int(record["S"])
        if actual_size != indexed_size:
            raise RuntimeError(
                f"{filename} has {actual_size} bytes, APKINDEX says {indexed_size}"
            )
        package_paths[name] = apk_path
        print(f"{filename} sha256={package['sha256']}")

    rebuild_rootfs(package_paths)
    write_source_metadata(records, package_paths)
    print(f"nginx sha256={sha256_file(ROOTFS / 'usr/sbin/nginx')}")
    print(f"Rootfs written to {ROOTFS}")
    print(f"Source metadata written to {SOURCE_JSON}")


if __name__ == "__main__":
    main()

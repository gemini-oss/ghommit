# - The key things to consider for reproducibility in this context:
#   - Docker base image, including its architecture
#   - Target architecture
#   - Rust version
#   - Zig version
#   - cargo-zigbuild version
#   - The SOURCE_DATE_EPOCH value
#   - The cargo profile being used (--release)
#   - The build directory

# - rust:1.87.0-bookworm for x86_64
#   - https://hub.docker.com/layers/library/rust/1.87.0-bookworm/images/sha256-510409508db9abe8be1f1bb6ca103bdea417564518c87b34494470c0cd322391
FROM rust@sha256:510409508db9abe8be1f1bb6ca103bdea417564518c87b34494470c0cd322391

# - Install dependencies
#   - minisign to verify Zig's signature
#   - cargo-zigbuild to use Zig's linker with cargo
#   - The x86_64-unknown-linux-musl target since it's not included by default
RUN apt update && \
    apt install -y \
        minisign && \
    cargo install cargo-zigbuild --version 0.20.0 && \
    rustup target add x86_64-unknown-linux-musl

# - Zig: Set up environment variables
#   - Note: Upgrading Zig will require changing not just environment variables,
#     but also the signature of the release archive

ENV ZIG_TEMP_DIR='/tmp/zig-setup'
ENV ZIG_ARCHITECTURE='x86_64'
ENV ZIG_VERSION='0.14.0'

ENV ZIG_PUBLIC_KEY_FILENAME='zig.minisign.pub'
ENV ZIG_ARCHIVE_FILENAME="zig-linux-${ZIG_ARCHITECTURE}-${ZIG_VERSION}.tar.xz"
ENV ZIG_ARCHIVE_SIGNATURE_FILENAME="${ZIG_ARCHIVE_FILENAME}.minisig"
ENV ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME="${ZIG_ARCHIVE_SIGNATURE_FILENAME}.base64"
ENV ZIG_ARCHIVE_URL="https://ziglang.org/download/${ZIG_VERSION}/${ZIG_ARCHIVE_FILENAME}"

# - Set up Zig: The rest
#   - Make ZIG_TEMP_DIR
#   - Change to ZIG_TEMP_DIR
#   - echo the public key to ZIG_PUBLIC_KEY_FILENAME
#   - echo the signature of the release archive as base64 into
#     ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME
#     - This is base64-encoded to avoid whitespace issues, but should match the
#       original at "${ZIG_ARCHIVE_URL}.minisig" when decoded
#   - echo the base64-decoded signature to ZIG_ARCHIVE_SIGNATURE_FILENAME
#   - Download the release archive
#   - Verify the archive's signature
#   - Make the /opt/zig directory
#   - Extract the archive to /opt/zig
#   - Symlink /opt/zig/zig to /usr/local/bin/zig
#   - Change directory to /
#   - Remove ZIG_TEMP_DIR
RUN \
    mkdir "${ZIG_TEMP_DIR}" && \
    cd "${ZIG_TEMP_DIR}" && \
    \
    echo 'untrusted comment: minisign public key'                    > "${ZIG_PUBLIC_KEY_FILENAME}" && \
    echo 'RWSGOq2NVecA2UPNdBUZykf1CCb147pkmdtYxgb3Ti+JO/wCYvhbAb/U' >> "${ZIG_PUBLIC_KEY_FILENAME}" && \
    \
    echo 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUlVT'  > "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'R09xMk5WZWNBMlFGZVlkLzNiOVpPN0NtMkNjSzZQT0cybFYzRHhSQzQ2MWdPa2JidzljOUxPcEsy' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'Y0MwUm1YVVBVK0toUm5sQllCVVJoRURRNjQyOHBWNE9iUVc5NWdjPQp0cnVzdGVkIGNvbW1lbnQ6' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'IHRpbWVzdGFtcDoxNzQxMTYwMDAzCWZpbGU6emlnLWxpbnV4LXg4Nl82NC0wLjE0LjAudGFyLnh6' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'CWhhc2hlZAoxbnVnN3lGT0huSjVtZG1qOEExeGpRVCtyd3VDc2Mxbi9paGNKWkxUQXF4aVEwSkUw' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'QWdHRFZQMjBoSFRCakVpays2VHl6aGJzTjFabTZyRWlqdFhEUT09Cg=='                     >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    base64 -d < "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" > "${ZIG_ARCHIVE_SIGNATURE_FILENAME}" && \
    \
    curl -fsSL "${ZIG_ARCHIVE_URL}" -o "${ZIG_ARCHIVE_FILENAME}" && \
    minisign -V -p "${ZIG_PUBLIC_KEY_FILENAME}" -m "${ZIG_ARCHIVE_FILENAME}" -x "${ZIG_ARCHIVE_SIGNATURE_FILENAME}" && \
    mkdir -p /opt/zig && \
    tar -xJf "${ZIG_ARCHIVE_FILENAME}" --strip-components=1 -C /opt/zig && \
    ln -s /opt/zig/zig /usr/local/bin/zig && \
    cd / && \
    rm -rf "${ZIG_TEMP_DIR}"

# - Set SOURCE_DATE_EPOCH for reproducible builds
ENV SOURCE_DATE_EPOCH='1715644800'

WORKDIR /build
CMD ["cargo", "zigbuild", "--release", "--target", "x86_64-unknown-linux-musl"]

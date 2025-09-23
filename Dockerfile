# - The key things to consider for reproducibility in this context:
#   - Docker base image, including its architecture
#   - Target architecture
#   - Rust version
#   - Zig version
#   - cargo-zigbuild version
#   - The SOURCE_DATE_EPOCH value
#   - The cargo profile being used (--release)
#   - The build directory

# - rust:1.90.0-bookworm for x86_64
#   - https://hub.docker.com/layers/library/rust/1.90.0-bookworm/images/sha256-e026148c928f4e5e9dfbc058c4ea189254de503926796bc473f9d2d25cc4690c
FROM --platform=linux/amd64 docker.io/library/rust@sha256:e026148c928f4e5e9dfbc058c4ea189254de503926796bc473f9d2d25cc4690c

# - Install dependencies
#   - minisign to verify Zig's signature
#   - cargo-zigbuild to use Zig's linker with cargo
#   - The x86_64-unknown-linux-musl target since it's not included by default
RUN apt update && \
    apt install -y \
        minisign && \
    cargo install cargo-zigbuild --version 0.20.1 && \
    rustup target add \
        aarch64-unknown-linux-musl \
        x86_64-unknown-linux-musl

# - Zig: Set up environment variables
#   - Note: Upgrading Zig will require changing not just environment variables,
#     but also the signature of the release archive

ENV ZIG_TEMP_DIR='/tmp/zig-setup'
ENV ZIG_ARCHITECTURE='x86_64'
ENV ZIG_VERSION='0.15.1'

ENV ZIG_PUBLIC_KEY_FILENAME='zig.minisign.pub'
ENV ZIG_ARCHIVE_FILENAME="zig-${ZIG_ARCHITECTURE}-linux-${ZIG_VERSION}.tar.xz"
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
    echo 'R09xMk5WZWNBMldrM05wdzJJR3pJQzhmTWVjNllPRmlLUm8zRjd0bWFFZ2RWTDh3azA2YW1DQTY0' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'RnhvQzhpTjYrRnlsbFV6K21RdGVvT1hZbEVZRmtaWVVCRzJCbmdvPQp0cnVzdGVkIGNvbW1lbnQ6' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'IHRpbWVzdGFtcDoxNzU1NzA3MTIxCWZpbGU6emlnLXg4Nl82NC1saW51eC0wLjE1LjEudGFyLnh6' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'CWhhc2hlZAp5bnNsWmhpK3FNMG85VkZXZEVXajAvV3NuMXp1VzR1Ykh5STZhNEFlSlcyRGRMcTA0' >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
    echo 'ZDBUUHVNTVdnbkZCS2N4ZmpoRmhCVW1hcXIxNTZkSWxJb0ZEQT09Cg=='                     >> "${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \
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

# - Set the default target
ENV TARGET_TRIPLE='x86_64-unknown-linux-musl'

WORKDIR /build
CMD ["/bin/sh", "-c", "cargo zigbuild --release --target ${TARGET_TRIPLE}"]

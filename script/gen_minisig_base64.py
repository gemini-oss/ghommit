"""
- Generate base64-encoded minisig signature for Zig archive for Dockerfile
  inclusion
- Examples:
  - python3 gen_minisig_base64.py 'https://ziglang.org/download/0.14.0/zig-linux-x86_64-0.14.0.tar.xz.minisig'
  - python3 gen_minisig_base64.py 'https://ziglang.org/download/0.15.1/zig-x86_64-linux-0.15.1.tar.xz.minisig'
"""
import base64
import sys
import urllib.request


CHUNK_SIZE = 76
GEN_DOCKER_TAIL = '"${ZIG_ARCHIVE_SIGNATURE_BASE64_FILENAME}" && \\'


def to_chunks(s: str) -> list[str]:
    return [s[i:i+CHUNK_SIZE] for i in range(0, len(s), CHUNK_SIZE)]


def gen_docker_line(chunk: str, is_first_line: bool = False) -> str:
    redirect = " >" if is_first_line else ">>"
    chunk_quoted = f"'{chunk}'"
    chunk_size_with_quotes = CHUNK_SIZE + 2

    return f"    echo {chunk_quoted:{chunk_size_with_quotes}} {redirect} {GEN_DOCKER_TAIL}"


def main():
    if len(sys.argv) != 2:
        print("Usage: python3 gen_minisig_base64.py <minisig_url>")
        sys.exit(1)
    else:
        url = sys.argv[1]

    with urllib.request.urlopen(url) as response:
        data = response.read()

    encoded = base64.b64encode(data).decode("ascii")
    chunks = to_chunks(encoded)

    for i, chunk in enumerate(chunks):
        is_first_line = i == 0
        docker_line = gen_docker_line(chunk, is_first_line)

        print(docker_line)


if __name__ == '__main__':
    main()

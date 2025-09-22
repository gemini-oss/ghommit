# ghommit

GitHub Commit: Create signed commits via GitHub's API

## Example usage

```bash
# 1. Set the staging area

git checkout -b ghommit-testing
echo foo >> foo
git add foo

# 2. Set the environment variables

export GHOMMIT_GITHUB_APP_ID='Fill this in'
export GHOMMIT_GITHUB_APP_INSTALLATION_ID='Fill this in'
export GHOMMIT_GITHUB_APP_PRIVATE_KEY_PEM_DATA='Fill this in'

# 3. Run ghommit to have the GitHub App create the commit
#    - Note that this is like `commit` and `push` in one command with the caveat
#      that the local git state will not be in sync since the commit is being
#      done remotely

ghommit -m 'Adding to foo'

# 4. (optional) If more git-related actions needs to be performed, keep in mind
#    that that git state is out of sync and may require syncing
```

## Building (basic)

```shell
# Building for the current machine
cargo build --release

# One-time setup for cross-compilation
cargo install cargo-zigbuild

# - Building for Linux on x86_64
rustup target add x86_64-unknown-linux-musl
cargo zigbuild --release --target x86_64-unknown-linux-musl
```

## Building (reproducible)

```sh
# - Build a build container image
podman build -t cargo-zigbuild:latest .

# - Build a release binary for aarch64-unknown-linux-musl
#   - Resultant executable will be located at
#     target/aarch64-unknown-linux-musl/release/ghommit
podman run -e TARGET_TRIPLE=aarch64-unknown-linux-musl -v "$PWD:/build" --rm cargo-zigbuild:latest

# - Build a release binary for x86_64-unknown-linux-musl
#   - Resultant executable will be located at
#     target/x86_64-unknown-linux-musl/release/ghommit
podman run -e TARGET_TRIPLE=x86_64-unknown-linux-musl -v "$PWD:/build" --rm cargo-zigbuild:latest
```

## Testing

- Note: Many of the integration tests are ignored by default because they
  require many environment variables to be set:
    - Normal environment variables:
        - `GHOMMIT_GITHUB_APP_ID`
        - `GHOMMIT_GITHUB_APP_INSTALLATION_ID`
        - `GHOMMIT_GITHUB_APP_PRIVATE_KEY_PEM_DATA`
    - Test environment variables
        - `GHOMMIT_TEST_BASE_TREE_ID`
        - `GHOMMIT_TEST_COMMIT_MESSAGE`
        - `GHOMMIT_TEST_GITHUB_REPO_OWNER`
        - `GHOMMIT_TEST_GITHUB_REPO_NAME`
        - `GHOMMIT_TEST_REPO_PATH`

```bash
# - Run the unit tests

cargo test --lib

# - Run the integration tests

cargo test --test '*'

# - Run all unignored tests

cargo test

# - Run the ignored tests

cargo test -- --ignored

# - Run all tests

cargo test
cargo test -- --ignored
```

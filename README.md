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

ghommit --github-owner-and-repo 'gemini/example_repo' -m 'Adding to foo'

# 4. (optional) If more git-related actions needs to be performed, keep in mind
#    that that git state is out of sync and may require syncing
```

## Building

### Building for the current Mac on the current Mac

```bash
# Produces target/release/ghommit
cargo build --release
```

## Testing

```bash
# - Integration tests require several environment variables to be set:
#   - Normal environment variables:
#     - GHOMMIT_GITHUB_APP_ID
#     - GHOMMIT_GITHUB_APP_INSTALLATION_ID
#     - GHOMMIT_GITHUB_APP_PRIVATE_KEY_PEM_DATA
#   - Test environment variables
#     - GHOMMIT_TEST_COMMIT_MESSAGE
#     - GHOMMIT_TEST_GITHUB_REPO_OWNER
#     - GHOMMIT_TEST_GITHUB_REPO_NAME
#     - GHOMMIT_TEST_REPO_PATH

# - Run the unit tests

cargo test --lib

# - Run the integration tests

cargo test --test '*'

# - Run all tests

cargo test
```

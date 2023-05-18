use std::env;
use std::sync::Arc;

use ghommit::config::{CommandLineArguments, Config, EnvironmentVariableConfig, GitConfig};
use ghommit::github::GitHubClient;
use ghommit::github::rest_api::{create_a_blob, create_a_tree};

struct EnvironmentVariableTestConfig {
    base_tree_id: String,
    commit_message: String,
    repo_path: String,
    github_repo_owner: String,
    github_repo_name: String,
}

impl EnvironmentVariableTestConfig {
    fn environment_variable(name: &str) -> String {
        match env::var(name) {
            Ok(result) => result,
            Err(_) => panic!("Environment variable not set: {}", name),
        }
    }

    pub fn gather() -> EnvironmentVariableTestConfig {
        EnvironmentVariableTestConfig {
            base_tree_id: Self::environment_variable("GHOMMIT_TEST_BASE_TREE_ID"),
            commit_message: Self::environment_variable("GHOMMIT_TEST_COMMIT_MESSAGE"),
            repo_path: Self::environment_variable("GHOMMIT_TEST_REPO_PATH"),
            github_repo_owner: Self::environment_variable("GHOMMIT_TEST_GITHUB_REPO_OWNER"),
            github_repo_name: Self::environment_variable("GHOMMIT_TEST_GITHUB_REPO_NAME"),
        }
    }
}

fn default_github_client() -> GitHubClient {
    let env_config = EnvironmentVariableConfig::gather().unwrap();

    GitHubClient::new(
        env_config.github_app_id,
        env_config.github_app_installation_id,
        env_config.github_app_private_key,
    )
}

fn default_config() -> Config {
    let test_config = EnvironmentVariableTestConfig::gather();

    let cli_args = CommandLineArguments {
        commit_message: "ghommit test message".to_string(),
        git_should_force_push: false,
        github_repo_owner: test_config.github_repo_owner,
        github_repo_name: test_config.github_repo_name,
    };
    let maybe_repo = git2::Repository::open(&test_config.repo_path);
    let git_config = GitConfig::gather(maybe_repo).unwrap();
    let env_config = EnvironmentVariableConfig::gather().unwrap();

    Config::from(cli_args, git_config, env_config)
}

#[test]
#[ignore]
fn access_token_caching() {
    let github_client = default_github_client();

    let expected = github_client.get_access_token(false).unwrap();
    let actual = github_client.get_access_token(false).unwrap();

    assert!(Arc::ptr_eq(&actual, &expected));
}

#[test]
#[ignore]
fn access_token_forcing() {
    let github_client = default_github_client();

    let access_token_1 = github_client.get_access_token(false).unwrap();
    let access_token_2 = github_client.get_access_token(true).unwrap();

    assert!(!Arc::ptr_eq(&access_token_1, &access_token_2));
}

#[test]
#[ignore]
fn create_a_blob_text() {
    let config = default_config();
    let github_client = default_github_client();

    let payload = create_a_blob::RequestBody {
        content: "hello",
        encoding: create_a_blob::Encoding::Utf8,
    };

    let response = github_client.create_a_blob(&config, &payload).unwrap();

    // printf 'hello' | git hash-object --stdin
    assert_eq!(response.sha, "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");
}

#[test]
#[ignore]
fn create_a_blob_binary() {
    let config = default_config();
    let github_client = default_github_client();

    // printf '\x80' | base64
    let first_invalid_utf8_byte_base64 = "gA==";
    // printf '\x80' | git hash-object --stdin
    let first_invalid_utf8_byte_git_hashed = "5416677bc7dab0c8bec3f5bf44d7d28b4ff73b13";

    let payload = create_a_blob::RequestBody {
        content: first_invalid_utf8_byte_base64,
        encoding: create_a_blob::Encoding::Base64,
    };

    let response = github_client.create_a_blob(&config, &payload).unwrap();

    assert_eq!(response.sha, first_invalid_utf8_byte_git_hashed);
}

#[test]
#[ignore]
fn create_a_commit() {
    assert!(false, "Not implemented")
}

#[test]
#[ignore]
fn create_a_reference() {
    assert!(false, "Not implemented")
}

#[test]
#[ignore]
fn create_a_tree() {
    let test_config = EnvironmentVariableTestConfig::gather();
    let config = default_config();
    let github_client = default_github_client();

    let payload = create_a_tree::RequestBody {
        base_tree: test_config.base_tree_id,
        tree: vec![
            create_a_tree::TreeNode {
                path: "foo".to_owned(),
                file_mode: create_a_tree::FileMode::Blob,
                node_type: create_a_tree::NodeType::Blob,
                sha_or_content: create_a_tree::ShaOrContent::Content("foo\n".to_owned()),
            },
        ],
    };

    // - The request succeeding is good enough for now
    github_client.create_a_tree(&config, &payload).unwrap();
}

#[test]
#[ignore]
fn get_a_reference() {
    assert!(false, "Not implemented")
}

#[test]
#[ignore]
fn update_a_reference() {
    assert!(false, "Not implemented")
}

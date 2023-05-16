use std::sync::Arc;

use ghommit::config::EnvironmentVariableConfig;
use ghommit::github::GitHubClient;

fn default_github_client() -> GitHubClient {
    let env_config = EnvironmentVariableConfig::gather().unwrap();

    GitHubClient::new(
        env_config.github_app_id,
        env_config.github_app_installation_id,
        env_config.github_app_private_key,
    )
}

#[test]
fn access_token_caching() {
    let github_client = default_github_client();

    let expected = github_client.get_access_token(false).unwrap();
    let actual = github_client.get_access_token(false).unwrap();

    assert!(Arc::ptr_eq(&actual, &expected));
}

#[test]
fn access_token_forcing() {
    let github_client = default_github_client();

    let access_token_1 = github_client.get_access_token(false).unwrap();
    let access_token_2 = github_client.get_access_token(true).unwrap();

    assert!(!Arc::ptr_eq(&access_token_1, &access_token_2));
}

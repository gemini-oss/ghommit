use std::collections::HashMap;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH, Duration};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{header::{HeaderMap, self, HeaderValue, HeaderName}, StatusCode};
use serde::{Serialize, Deserialize};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";

pub struct GitHubClient<'a> {
    pub github_app_id: u64,
    pub github_app_installation_id: u64,
    pub github_app_private_key: &'a EncodingKey,
}

/// https://docs.github.com/en/graphql/reference/input-objects#fileaddition
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAddition {
    pub path: String,
    pub contents: String,
}

/// https://docs.github.com/en/graphql/reference/input-objects#filedeletion
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDeletion {
    pub path: String,
}

/// https://docs.github.com/en/graphql/reference/input-objects#filechanges
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChanges {
    pub additions: Vec<FileAddition>,
    pub deletions: Vec<FileDeletion>,
}

/// There is a second representation for this, but this implementation ignores
/// it.
///
/// https://docs.github.com/en/graphql/reference/input-objects#committablebranch
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommittableBranch {
    pub repository_name_with_owner: String,
    pub branch_name: String,
}

/// https://docs.github.com/en/graphql/reference/input-objects#commitmessage
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMessage {
    pub body: Option<String>,
    pub headline: String,
}

/// https://docs.github.com/en/graphql/reference/input-objects#createcommitonbranchinput
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommitOnBranchInput {
    pub branch: CommittableBranch,
    pub client_mutation_id: Option<String>,
    pub expected_head_oid: String,
    pub file_changes: Option<FileChanges>,
    pub message: CommitMessage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommitOnBranchInputWrapper {
    input: CreateCommitOnBranchInput,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommitOnBranchInputFull {
    query: String,
    variables: CreateCommitOnBranchInputWrapper,
}

#[derive(Debug, Serialize)]
struct Claims {
    iat: usize,
    exp: usize,
    iss: String,
}

/// Abbreviated representation of the access token response body
/// https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app
#[derive(Debug, Deserialize)]
struct GitHubAccessTokenResponse {
    token: String,
}

impl GitHubClient<'_> {
    fn unix_epoch_second_now() -> Result<usize, String> {
        let now = SystemTime::now();

        match now.duration_since(UNIX_EPOCH) {
            Ok(duration) => Ok(duration.as_secs() as usize),
            Err(e) => Err(format!("Impossible duration from Unix epoch to now: {} nanoseconds", e.duration().as_nanos())),
        }
    }

    fn get_http_client(&self, maybe_timeout_seconds: Option<u64>) -> Result<reqwest::blocking::Client, String> {
        let timeout_seconds = match maybe_timeout_seconds {
            Some(timeout_seconds) => timeout_seconds,
            None => 60,
        };

        let maybe_client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(timeout_seconds)).build();

        match maybe_client {
            Ok(client) => Ok(client),
            Err(_) => Err("Unable to create HTTP client".to_string()),
        }
    }

    fn get_jwt(&self) -> Result<String, String> {
        let now = Self::unix_epoch_second_now()?;
        let ten_minutes_from_now = now + (10 * 60);

        if ten_minutes_from_now < now {
            return Err(format!("Adding ten minutes to now in seconds ({}) resulted in time that is less than now ({})", now, ten_minutes_from_now));
        }

        let claims = Claims {
            iat: now,
            exp: ten_minutes_from_now,
            iss: self.github_app_id.to_string(),
        };

        let maybe_jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.github_app_private_key);

        match maybe_jwt {
            Ok(jwt) => Ok(jwt),
            Err(e) => Err(format!("Unable to create JWT: {}", e.to_string())),
        }
    }

    /// https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app
    fn get_access_token(&self) -> Result<String, String> {
        fn generate_headers(jwt: &str) -> Result<HeaderMap, String> {
            // - `HeaderName`s will panic on instantiation if not lowercase
            let github_api_header_name_str = "X-GitHub-Api-Version".to_lowercase();

            let github_api_header_name = match HeaderName::from_str(&github_api_header_name_str) {
                Ok(name) => name,
                Err(_) => Err(format!("Unable to create header named {}", github_api_header_name_str))?,
            };

            let auth_header_value = match HeaderValue::from_str(&format!("Bearer {}", jwt)) {
                Ok(value) => value,
                Err(_) => Err("Unable to create header value with JWT string")?,
            };

            let mut headers = HeaderMap::new();

            headers.insert(header::ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
            headers.insert(header::AUTHORIZATION, auth_header_value);
            headers.insert(github_api_header_name, HeaderValue::from_static("2022-11-28"));
            // - GitHub requires the User-Agent header
            //   - https://docs.github.com/en/rest/overview/resources-in-the-rest-api#user-agent-required
            headers.insert(header::USER_AGENT, HeaderValue::from_static("ghommit"));

            Ok(headers)
        }

        let url = format!("https://api.github.com/app/installations/{}/access_tokens", self.github_app_installation_id);
        let jwt = self.get_jwt()?;
        let headers = generate_headers(&jwt)?;

        let client = self.get_http_client(Some(10))?;

        let request = client.post(url).headers(headers);

        let response = match request.send() {
            Ok(response) => response,
            Err(e) => Err(format!("Error occurred while acquiring access token: {}", e.to_string()))?,
        };

        if response.status() == StatusCode::CREATED {
            match response.json::<GitHubAccessTokenResponse>() {
                Ok(access_token_res) => {
                    Ok(access_token_res.token)
                },
                Err(_) => {
                    Err("Unable to deserialize access token".to_owned())
                },
            }
        } else {
            let status_code = response.status();

            match &response.text() {
                Ok(text) => Err(format!("Unexpected status code {} while acquiring access token: {}", status_code, text)),
                Err(_) => Err(format!("Unexpected status code {} while acquiring access token; body could not be decoded as text", status_code)),
            }
        }
    }

    pub fn create_commit_on_branch(&self, args: CreateCommitOnBranchInput) -> Result<(), String> {
        fn generate_headers(jwt: &str) -> Result<HeaderMap, String> {
            let auth_header_value = match HeaderValue::from_str(&format!("Bearer {}", jwt)) {
                Ok(value) => value,
                Err(_) => Err("Unable to create header value with JWT string")?,
            };

            let mut headers = HeaderMap::new();

            headers.insert(header::AUTHORIZATION, auth_header_value);
            // - GitHub requires the User-Agent header
            //   - https://docs.github.com/en/rest/overview/resources-in-the-rest-api#user-agent-required
            headers.insert(header::USER_AGENT, HeaderValue::from_static("ghommit"));

            Ok(headers)
        }

        let mutation = r#"
            mutation ($input: CreateCommitOnBranchInput!) {
              createCommitOnBranch(input: $input) {
                commit {
                  url
                }
              }
            }
        "#;

        let payload = CreateCommitOnBranchInputFull {
            query: mutation.to_owned(),
            variables: CreateCommitOnBranchInputWrapper {
                input: args,
            }
        };

        let http_client = self.get_http_client(None)?;

        let jwt = self.get_access_token()?;
        let headers = generate_headers(&jwt)?;

        let request = http_client.post(GRAPHQL_URL).headers(headers).json(&payload);

        let response = match request.send() {
            Ok(response) => response,
            Err(e) => Err(format!("Request failed: {}", e.to_string()))?,
        };

        let response_json: HashMap<String, serde_json::Value> = match response.json() {
            Ok(json) => json,
            Err(e) => Err(format!("Unable to convert response to JSON: {}", e.to_string()))?,
        };

        match response_json.get("errors") {
            Some(errors) => Err(format!("Errors were encountered: {}", errors))?,
            None => {},
        }

        println!("{:?}", response_json);

        // TODO: Deserialize as expected return

        Ok(())
    }
}

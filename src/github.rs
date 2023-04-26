use std::time::{SystemTime, Duration, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use reqwest::blocking::Response;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::Config;

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const REST_API_BASE_URL: &str = "https://api.github.com";

pub struct GitHubClient<'a> {
    pub github_app_id: u64,
    pub github_app_installation_id: u64,
    pub github_app_private_key: &'a EncodingKey,
}

#[derive(Debug, Serialize)]
struct Claims {
    iat: usize,
    exp: usize,
    iss: String,
}

mod custom_header {
    use reqwest::header::HeaderName;

    // Note: Header names must be lowercase or reqwest will panic
    pub const X_GITHUB_API_VERSION: HeaderName = HeaderName::from_static("x-github-api-version");
}

enum GitHubApiType {
    GraphQL,
    REST,
}

enum AuthorizationTokenType {
    AccessToken,
    JWT,
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

    /// https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app
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
        let url = format!("{}/app/installations/{}/access_tokens", REST_API_BASE_URL, self.github_app_installation_id);

        let response = self.make_api_request::<()>(&url, None, Some(AuthorizationTokenType::JWT))?;

        let status_code = response.status();

        match status_code {
            StatusCode::CREATED => match response.json::<response::GitHubAccessToken>() {
                Ok(access_token_res) => Ok(access_token_res.token),
                Err(_) => Err("Unable to deserialize access token".to_owned()),
            }
            _ => match response.text() {
                Ok(text) => Err(format!("Unexpected status code {} while acquiring access token: {}", status_code, text)),
                Err(_) => Err(format!("Unexpected status code {} while acquiring access token; body could not be decoded as text", status_code)),
            }
        }
    }

    /// Returns headers with the following headers pre-set:
    ///
    /// - `Accept`
    /// - `Authorization`
    /// - `User-Agent`
    /// - `X-GitHub-Api-Version` (if using the REST API)
    fn base_headers(&self, api_type: GitHubApiType, auth_token_type: AuthorizationTokenType) -> Result<HeaderMap, String> {
        let token_str = match auth_token_type {
            AuthorizationTokenType::AccessToken => self.get_access_token()?,
            AuthorizationTokenType::JWT => self.get_jwt()?,
        };

        let auth_header_value = match HeaderValue::from_str(&format!("Bearer {}", token_str)) {
            Ok(value) => value,
            Err(_) => Err("Unable to create header value with JWT string")?,
        };

        let accept_header_value = match api_type {
            GitHubApiType::GraphQL => HeaderValue::from_static("application/json"),
            GitHubApiType::REST => HeaderValue::from_static("application/vnd.github+json"),
        };

        let mut headers = HeaderMap::new();

        headers.insert(
            header::ACCEPT,
            accept_header_value,
        );
        headers.insert(
            header::AUTHORIZATION,
            auth_header_value,
        );
        headers.insert(
            // - GitHub requires the User-Agent header
            //   - https://docs.github.com/en/rest/overview/resources-in-the-rest-api#user-agent-required
            header::USER_AGENT,
            HeaderValue::from_static("ghommit")
        );

        match api_type {
            GitHubApiType::GraphQL => {}
            GitHubApiType::REST => {
                headers.insert(
                    custom_header::X_GITHUB_API_VERSION,
                    HeaderValue::from_static("2022-11-28"),
                );
            }
        }

        Ok(headers)
    }

    fn make_api_request<T: Serialize + ?Sized>(&self, url: &str, json: Option<&T>, auth_token_type: Option<AuthorizationTokenType>) -> Result<Response, String> {
        let auth_token_type = match auth_token_type {
            Some(auth_token_type) => auth_token_type,
            None => AuthorizationTokenType::AccessToken,
        };

        let http_client = self.get_http_client(None)?;
        let headers = self.base_headers(GitHubApiType::REST, auth_token_type)?;
        let request = http_client.post(url).headers(headers);

        let request = match json {
            Some(json) => request.json(&json),
            None => request
        };

        match request.send() {
            Ok(response) => Ok(response),
            Err(e) => Err(format!("Request failed: {}", e.to_string())),
        }
    }

    fn make_graphql_request<T: Serialize + ?Sized, R: DeserializeOwned>(&self, json: &T) -> Result<R, String> {
        let http_client = self.get_http_client(None)?;
        let headers = self.base_headers(GitHubApiType::GraphQL, AuthorizationTokenType::AccessToken)?;
        let request = http_client.post(GRAPHQL_URL).headers(headers).json(&json);

        let response = match request.send() {
            Ok(response) => response,
            Err(e) => Err(format!("Request failed: {}", e.to_string()))?,
        };

        match response.json() {
            Ok(response) => Ok(response),
            Err(e) => Err(format!("Error occurred while deserializing GraphQL request: {}", e.to_string())),
        }
    }

    fn does_branch_exist(&self, config: &Config) -> Result<bool, String> {
        let query = r#"
            query ($owner: String!, $repoName: String!, $branchName: String!) {
                repository(owner: $owner, name: $repoName) {
                    ref(qualifiedName: $branchName) {
                        name
                    }
                }
            }
        "#;

        let payload = request::DoesBranchExist {
            query: query,
            variables: request::DoesBranchExistVariables {
                owner: &config.github_repo_owner,
                repo_name: &config.github_repo_name,
                branch_name: &config.git_branch_name,
            },
        };

        let response: response::DoesBranchExist = self.make_graphql_request(&payload)?;
        let branch_exists = response.data.repository.reference.is_some();

        Ok(branch_exists)
    }

    /// https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#create-a-reference
    fn create_branch(&self, config: &Config) -> Result<(), String> {
        let url = format!("{}/repos/{}/{}/git/refs", REST_API_BASE_URL, config.github_repo_owner, config.github_repo_name);

        let payload = request::CreateBranch {
            reference: &format!("refs/heads/{}", config.git_branch_name),
            sha: &config.git_head_object_id,
        };

        let response = self.make_api_request(&url, Some(&payload), None)?;

        let status_code = response.status();

        match status_code {
            StatusCode::CREATED => Ok(()),
            _ => {
                match &response.text() {
                    Ok(text) => Err(format!("Unexpected status code {} while creating branch: {}", status_code, text)),
                    Err(_) => Err(format!("Unexpected status code {} while creating branch; body could not be decoded as text", status_code)),
                }
            }
        }
    }

    fn ensure_branch_exists(&self, config: &Config) -> Result<(), String> {
        if self.does_branch_exist(config)? {
            Ok(())
        } else {
            self.create_branch(config)
        }
    }

    /// Creates a commit on a branch. Returns the URL of the commit.
    pub fn create_commit_on_branch(&self, config: &Config, args: request::CreateCommitOnBranchInput) -> Result<String, String> {
        // - `createCommitOnBranch` fails if the branch doesn't exist, so ensure
        //   that it exists first
        self.ensure_branch_exists(config)?;

        let mutation = r#"
            mutation ($input: CreateCommitOnBranchInput!) {
                createCommitOnBranch(input: $input) {
                    commit {
                        url
                    }
                }
            }
        "#;

        let payload = request::CreateCommitOnBranch {
            query: mutation.to_owned(),
            variables: request::CreateCommitOnBranchVariables {
                input: args,
            },
        };

        let response_data: response::CreateCommitOnBranch = self.make_graphql_request(&payload)?;

        Ok(response_data.data.create_commit_on_branch.commit.url)
    }
}

pub mod request {
    use serde::Serialize;

    // > CreateBranch

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateBranch<'a> {
        #[serde(rename = "ref")]
        pub reference: &'a str,
        pub sha: &'a str,
    }

    // > CreateCommitOnBranch

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranch {
        pub query: String,
        pub variables: CreateCommitOnBranchVariables,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranchVariables {
        pub input: CreateCommitOnBranchInput,
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

    /// https://docs.github.com/en/graphql/reference/input-objects#commitmessage
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CommitMessage {
        pub body: Option<String>,
        pub headline: String,
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

    /// https://docs.github.com/en/graphql/reference/input-objects#filechanges
    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FileChanges {
        pub additions: Vec<FileAddition>,
        pub deletions: Vec<FileDeletion>,
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

    // > DoesBranchExist

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExist<'a> {
        pub query: &'a str,
        pub variables: DoesBranchExistVariables<'a>,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExistVariables<'a> {
        pub owner: &'a str,
        pub repo_name: &'a str,
        pub branch_name: &'a str,
    }
}

mod response {
    use serde::Deserialize;

    // > CreateCommitOnBranch

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranch {
        pub data: CreateCommitOnBranchData,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranchData {
        pub create_commit_on_branch: CreateCommitOnBranchCreateCommitOnBranch,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranchCreateCommitOnBranch {
        pub commit: CreateCommitOnBranchCommit,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateCommitOnBranchCommit {
        pub url: String,
    }

    // > DoesBranchExist

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExist {
        pub data: DoesBranchExistData,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExistData {
        pub repository: DoesBranchExistRepository,
    }
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExistRepository {
        #[serde(rename = "ref")]
        pub reference: Option<DoesBranchExistName>,
    }
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DoesBranchExistName {
        pub name: String,
    }

    // > GetGitHubAccessToken

    /// Abbreviated representation of the access token response body
    /// https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app
    #[derive(Debug, Deserialize)]
    pub struct GitHubAccessToken {
        pub token: String,
    }
}

use std::time::{SystemTime, Duration, UNIX_EPOCH};
use std::sync::{Arc, Mutex};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use reqwest::blocking::Response;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::Config;
use crate::github::rest_api::create_an_installation_access_token;

use self::rest_api::create_a_blob;

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const REST_API_BASE_URL: &str = "https://api.github.com";

struct AccessToken {
    token: Arc<String>,
}

impl AccessToken {
    fn expires_soon(&self) -> bool {
        false
    }
}

pub struct GitHubClient {
    github_app_id: u64,
    github_app_installation_id: u64,
    github_app_private_key: EncodingKey,
    github_access_token: Mutex<Option<AccessToken>>,
}

#[derive(Debug, Serialize)]
struct Claims {
    iat: usize,
    exp: usize,
    iss: String,
}

mod custom_header {
    use once_cell::sync::Lazy;
    use reqwest::header::HeaderName;

    // Note: Header names must be lowercase or reqwest will panic
    pub static X_GITHUB_API_VERSION: Lazy<HeaderName> = Lazy::new(|| {
        HeaderName::from_static("x-github-api-version")
    });
}

enum GitHubApiType {
    GraphQL,
    Rest,
}

enum AuthorizationTokenType {
    AccessToken,
    Jwt,
}

impl GitHubClient {
    pub fn new(github_app_id: u64, github_app_installation_id: u64, github_app_private_key: EncodingKey) -> GitHubClient {
        GitHubClient {
            github_app_id: github_app_id,
            github_app_installation_id: github_app_installation_id,
            github_app_private_key: github_app_private_key,
            github_access_token: Mutex::new(None),
        }
    }

    fn unix_epoch_second_now() -> Result<usize, String> {
        let now = SystemTime::now();

        match now.duration_since(UNIX_EPOCH) {
            Ok(duration) => Ok(duration.as_secs() as usize),
            Err(e) => Err(format!("Impossible duration from Unix epoch to now: {} nanoseconds", e.duration().as_nanos())),
        }
    }

    fn get_http_client(&self, maybe_timeout_seconds: Option<u64>) -> Result<reqwest::blocking::Client, String> {
        let timeout_seconds = maybe_timeout_seconds.unwrap_or(60);

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
            Err(e) => Err(format!("Unable to create JWT: {}", e)),
        }
    }

    /// https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app
    pub fn get_access_token(&self, force_token_renewal: bool) -> Result<Arc<String>, String> {
        fn acquire_access_token(this: &GitHubClient) -> Result<create_an_installation_access_token::ResponseBody, String> {
            let url = format!("{}/app/installations/{}/access_tokens", REST_API_BASE_URL, this.github_app_installation_id);
            let response = this.make_api_request::<()>(&url, None, Some(AuthorizationTokenType::Jwt))?;
            GitHubClient::deserialize_expected_response(&this, response, &StatusCode::CREATED, "acquire an access token")
        }

        match self.github_access_token.lock() {
            Ok(mut access_token_guard) => {
                let should_update = force_token_renewal || match &*access_token_guard {
                    Some(token) => token.expires_soon(),
                    None => true,
                };

                if should_update {
                    let raw_access_token = acquire_access_token(self)?;

                    let access_token = AccessToken {
                        token: Arc::new(raw_access_token.token),
                    };

                    let return_token = access_token.token.clone();

                    *access_token_guard = Some(access_token);

                    Ok(return_token)
                } else {
                    match &*access_token_guard {
                        Some(access_token) => Ok(Arc::clone(&access_token.token)),
                        None => Err("Unexpected state: Access token is None".to_string()),
                    }
                }
            },
            Err(e) => Err(format!("Mutex poisoned unexpectedly: {}", e)),
        }
    }

    /// Returns headers with the following headers pre-set:
    ///
    /// - `Accept`
    /// - `Authorization`
    /// - `User-Agent`
    /// - `X-GitHub-Api-Version` (if using the REST API)
    fn base_headers(&self, api_type: GitHubApiType, auth_token_type: AuthorizationTokenType) -> Result<HeaderMap, String> {
        let token = match auth_token_type {
            AuthorizationTokenType::AccessToken => self.get_access_token(false)?,
            // - Creating an `Arc` is generally cheaper than cloning a `String`,
            //   so simply wrap the JWT `String` in an `Arc`
            AuthorizationTokenType::Jwt => Arc::new(self.get_jwt()?),
        };

        let auth_header_value = match HeaderValue::from_str(&format!("Bearer {}", token)) {
            Ok(value) => value,
            Err(_) => Err("Unable to create header value with JWT string")?,
        };

        let accept_header_value = match api_type {
            GitHubApiType::GraphQL => HeaderValue::from_static("application/json"),
            GitHubApiType::Rest => HeaderValue::from_static("application/vnd.github+json"),
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
            GitHubApiType::Rest => {
                headers.insert(
                    &*custom_header::X_GITHUB_API_VERSION,
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
        let headers = self.base_headers(GitHubApiType::Rest, auth_token_type)?;
        let request = http_client.post(url).headers(headers);

        let request = match json {
            Some(json) => request.json(&json),
            None => request
        };

        match request.send() {
            Ok(response) => Ok(response),
            Err(e) => Err(format!("Request failed: {}", e)),
        }
    }

    fn deserialize_expected_response<R: DeserializeOwned>(&self, response: Response, expected_status_code: &StatusCode, operation: &str) -> Result<R, String> {
        let status_code = response.status();

        if &status_code != expected_status_code {
            return match &response.text() {
                Ok(text) => Err(format!("Unexpected status code {} while {}: {}", status_code, operation, text)),
                Err(_) => Err(format!("Unexpected status code {} while {}; body could not be decoded as text", status_code, operation)),
            }
        }

        // - Read as text before deserializing to a struct since `.text()` and
        //   `.json()` are move operations, and `.text()` is more likely to
        //   succeed
        let text = match response.text() {
            Ok(text) => text,
            Err(e) => Err(format!("Error occurred while reading response body as text while trying to {}: {}", operation, e))?,
        };

        let data = match serde_json::from_str::<R>(&text) {
            Ok(typed_result) => typed_result,
            Err(e) => {
                let err_str = e.to_string();
                let type_str = std::any::type_name::<R>();
                Err(format!("Error occurred while deserializing response to {} while trying to {}: {}: {}", type_str, operation, err_str, text))?
            }
        };

        Ok(data)
    }

    fn make_graphql_request<T: Serialize + ?Sized, R: DeserializeOwned>(&self, json: &T) -> Result<R, String> {
        let http_client = self.get_http_client(None)?;
        let headers = self.base_headers(GitHubApiType::GraphQL, AuthorizationTokenType::AccessToken)?;
        let request = http_client.post(GRAPHQL_URL).headers(headers).json(&json);

        let response = match request.send() {
            Ok(response) => response,
            Err(e) => Err(format!("Request failed: {}", e))?,
        };

        // - Read as text before deserializing to a struct since `.text()` and
        //   `.json()` are move operations, and `.text()` is more likely to
        //   succeed
        let text = match response.text() {
            Ok(text) => text,
            Err(e) => Err(format!("Error occurred while reading GraphQL response body as text: {}", e))?,
        };

        let data = match serde_json::from_str::<R>(&text) {
            Ok(typed_result) => typed_result,
            Err(e) => {
                let err_str = e.to_string();
                let type_str = std::any::type_name::<R>();
                Err(format!("Error occurred while deserializing GraphQL response to {}: {}: {}", type_str, err_str, text))?
            }
        };

        Ok(data)
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

    /// [Create a blob](https://docs.github.com/en/rest/git/blobs?apiVersion=2022-11-28#create-a-blob)
    pub fn create_a_blob(&self, config: &Config, payload: &create_a_blob::RequestBody) -> Result<create_a_blob::ResponseBody, String> {
        let url = format!("{},/repos/{}/{}/git/blobs", REST_API_BASE_URL, config.github_repo_owner, config.github_repo_name);

        let response = self.make_api_request(&url, Some(&payload), None)?;

        self.deserialize_expected_response(response, &StatusCode::CREATED, "create a blob")
    }
}

pub mod rest_api {
    /// [Create a blob](https://docs.github.com/en/rest/git/blobs?apiVersion=2022-11-28#create-a-blob)
    pub mod create_a_blob {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Serialize)]
        pub enum Encoding {
            #[serde(rename = "base64")]
            Base64,
            #[serde(rename = "utf-8")]
            Utf8,
        }

        #[derive(Debug, Serialize)]
        pub struct RequestBody<'a> {
            pub content: &'a str,
            pub encoding: Encoding,
        }

        #[derive(Debug, Deserialize, Serialize)]
        pub struct ResponseBody {
            pub url: String,
            pub sha: String,
        }
    }

    pub mod create_an_installation_access_token {
        use serde::Deserialize;

        /// [Create an installation access token for an app](https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app)
        ///
        /// Abbreviated representation of the response body
        #[derive(Debug, Deserialize)]
        pub struct ResponseBody {
            pub token: String,
        }
    }

    /// [Create a tree](https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree)
    ///
    /// The entirety of GitHub's trees API uses snake case, so serde
    /// renaming is only necessary for enum variants that derive `Serialize`
    pub mod create_a_tree {
        use serde::{Serialize, Serializer};
        use serde::ser::SerializeStruct;

        #[derive(Debug, Serialize)]
        pub struct RequestBody {
            pub base_tree: String,
            pub tree: Vec<TreeNode>,
        }

        #[derive(Debug, Serialize)]
        pub enum FileMode {
            #[serde(rename = "100644")]
            Blob,
            #[serde(rename = "100755")]
            BlobExecutable,
            #[serde(rename = "160000")]
            Commit,
            #[serde(rename = "120000")]
            Link,
            #[serde(rename = "040000")]
            Tree,
        }

        #[derive(Debug, Serialize)]
        #[serde(rename_all = "lowercase")]
        pub enum NodeType {
            Blob,
            Commit,
            Tree,
        }

        // - Since `TreeNode` needs to manually implement serialization for
        //   this type, there is no need for anything serde-related
        #[derive(Debug)]
        pub enum ShaOrContent {
            Content(String),
            Sha(Option<String>),
        }

        #[derive(Debug)]
        pub struct TreeNode {
            pub path: String,
            pub file_mode: FileMode,
            pub node_type: NodeType,
            pub sha_or_content: ShaOrContent,
        }

        // - Since `sha_or_content` needs to serialize to one and only one
        //   of `sha` or `content`, which serde doesn't support, serialize
        //   must be implemented manually
        impl Serialize for TreeNode {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer
            {
                let mut state = serializer.serialize_struct("TreeNode", 4)?;

                state.serialize_field("path", &self.path)?;
                state.serialize_field("mode", &self.file_mode)?;
                state.serialize_field("type", &self.node_type)?;

                match &self.sha_or_content {
                    ShaOrContent::Sha(sha) => state.serialize_field("sha", sha)?,
                    ShaOrContent::Content(content) => state.serialize_field("content", content)?,
                };

                state.end()
            }
        }
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

pub mod response {
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
}

#[cfg(test)]
mod test_util {
    use serde_json::Value;

    /// Serialized strings may not have the same order, so deserialize and
    /// compare
    pub fn assert_eq_deserialized(a: &str, b: &str) {
        let a_deserialized: Value = serde_json::from_str(&a).unwrap();
        let b_deserialized: Value = serde_json::from_str(&b).unwrap();

        assert_eq!(a_deserialized, b_deserialized);
    }

    pub fn quote(s: &str) -> String {
        format!("\"{}\"", s)
    }

}
#[cfg(test)]
mod create_a_blob_tests {
    use super::rest_api::create_a_blob::{Encoding, RequestBody, ResponseBody};
    use super::test_util::{assert_eq_deserialized, quote};

    #[test]
    fn encoding_serialization() {
        let encodings = vec![
            Encoding::Base64,
            Encoding::Utf8,
        ];

        for encoding in encodings {
            let expected_raw = match encoding {
                Encoding::Base64 => "base64",
                Encoding::Utf8 => "utf-8",
            };

            let actual = serde_json::to_string(&encoding).unwrap();
            let expected = quote(expected_raw);

            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn create_a_blob_serialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/blobs?apiVersion=2022-11-28#create-a-blob
        let expected = r#"{"content":"Content of the blob","encoding":"utf-8"}"#;

        let actual_payload = RequestBody {
            content: "Content of the blob",
            encoding: Encoding::Utf8,
        };

        let actual = serde_json::to_string(&actual_payload).unwrap();

        assert_eq_deserialized(&actual, expected);
    }

    #[test]
    fn create_a_blob_deserialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/blobs?apiVersion=2022-11-28#create-a-blob
        let expected = r#"
        {
          "url": "https://api.github.com/repos/octocat/example/git/blobs/3a0f86fb8db8eea7ccbb9a95f325ddbedfb25e15",
          "sha": "3a0f86fb8db8eea7ccbb9a95f325ddbedfb25e15"
        }
        "#;

        let actual_payload = ResponseBody {
            url: "https://api.github.com/repos/octocat/example/git/blobs/3a0f86fb8db8eea7ccbb9a95f325ddbedfb25e15".to_string(),
            sha: "3a0f86fb8db8eea7ccbb9a95f325ddbedfb25e15".to_string(),
        };

        let actual = serde_json::to_string(&actual_payload).unwrap();

        assert_eq_deserialized(&actual, expected);
    }
}

#[cfg(test)]
mod create_a_tree_tests {
    use super::rest_api::create_a_tree::{FileMode, NodeType, RequestBody, ShaOrContent, TreeNode};
    use super::test_util::{assert_eq_deserialized, quote};

    fn manual_file_mode_to_json_string(file_mode: &FileMode) -> String {
        let git2_file_mode = match file_mode {
            FileMode::Blob => git2::FileMode::Blob,
            FileMode::BlobExecutable => git2::FileMode::BlobExecutable,
            FileMode::Commit => git2::FileMode::Commit,
            FileMode::Link => git2::FileMode::Link,
            FileMode::Tree => git2::FileMode::Tree,
        };

        let raw_mode_u32 = u32::from(git2_file_mode);
        let raw_mode_str = format!("{:0>6o}", raw_mode_u32);

        quote(&raw_mode_str)
    }

    fn manual_node_type_to_json_string(node_type: &NodeType) -> &'static str {
        match node_type {
            NodeType::Blob => "\"blob\"",
            NodeType::Commit => "\"commit\"",
            NodeType::Tree => "\"tree\"",
        }
    }

    /// Note: This is not all-encompassing as it only covers the two-character
    /// escape sequences as outlined in
    /// [the JSON spec](https://www.rfc-editor.org/rfc/rfc7159#section-7).
    fn manual_str_to_json_string(s: &str) -> String {
        let s = s
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\u{0008}", "\\b")
            .replace("\u{000C}", "\\f")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
            .replace("\t", "\\t");

        quote(&s)
    }

    fn manual_tree_node_to_json_string(path: &str, mode: &FileMode, node_type: &NodeType, sha_or_content: &ShaOrContent) -> String {
        let path_json_str = quote(path);
        let node_type_json_str = manual_node_type_to_json_string(&node_type);
        let mode_json_str = manual_file_mode_to_json_string(mode);

        let (sha_or_content_key, sha_or_content_value) = match sha_or_content {
            ShaOrContent::Content(content) => {
                ("content", manual_str_to_json_string(content))
            },
            ShaOrContent::Sha(maybe_sha) => {
                let value = match maybe_sha {
                    Some(sha) => manual_str_to_json_string(sha),
                    None => "null".to_owned(),
                };

                ("sha", value)
            },
        };

        format!(
            "{{{}:{},{}:{},{}:{},{}:{}}}",
            quote("path"), path_json_str,
            quote("mode"), mode_json_str,
            quote("type"), node_type_json_str,
            quote(sha_or_content_key), sha_or_content_value
        )
    }

    #[test]
    fn file_mode_serialization() {
        let file_modes = vec![
            FileMode::Blob,
            FileMode::BlobExecutable,
            FileMode::Commit,
            FileMode::Link,
        ];

        for file_mode in file_modes {
            let git2_file_mode = match file_mode {
                FileMode::Blob => git2::FileMode::Blob,
                FileMode::BlobExecutable => git2::FileMode::BlobExecutable,
                FileMode::Commit => git2::FileMode::Commit,
                FileMode::Link => git2::FileMode::Link,
                FileMode::Tree => git2::FileMode::Tree,
            };

            let raw_mode = u32::from(git2_file_mode);
            let expected_raw = format!("{:0>6o}", raw_mode);

            let actual = serde_json::to_string(&file_mode).unwrap();
            let expected = quote(&expected_raw);

            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn tree_node_serialization_with_content() {
        let path = "hello_world.txt";
        let mode = FileMode::Blob;
        let node_type = NodeType::Blob;
        let content = ShaOrContent::Content("hello world\n".to_owned());

        let expected = manual_tree_node_to_json_string(path, &mode, &node_type, &content);

        let tree_node = TreeNode {
            path: path.to_owned(),
            file_mode: mode,
            node_type: node_type,
            sha_or_content: content,
        };

        let actual = serde_json::to_string(&tree_node).unwrap();

        assert_eq_deserialized(&actual, &expected);
    }

    #[test]
    fn tree_node_serialization_with_sha() {
        let path = "hello_world.txt";
        let mode = FileMode::Blob;
        let node_type = NodeType::Blob;
        let sha = ShaOrContent::Sha(Some("0000000000000000000000000000000000000000".to_owned()));

        let expected = manual_tree_node_to_json_string(path, &mode, &node_type, &sha);

        let tree_node = TreeNode {
            path: path.to_owned(),
            file_mode: mode,
            node_type: node_type,
            sha_or_content: sha,
        };

        let actual = serde_json::to_string(&tree_node).unwrap();

        assert_eq_deserialized(&actual, &expected);
    }

    #[test]
    fn tree_node_serialization_with_no_sha() {
        let path = "hello_world.txt";
        let mode = FileMode::Blob;
        let node_type = NodeType::Blob;
        let sha = ShaOrContent::Sha(None);

        let expected = manual_tree_node_to_json_string(path, &mode, &node_type, &sha);

        let tree_node = TreeNode {
            path: path.to_owned(),
            file_mode: mode,
            node_type: node_type,
            sha_or_content: sha,
        };

        let actual = serde_json::to_string(&tree_node).unwrap();

        assert_eq_deserialized(&actual, &expected);
    }

    #[test]
    fn node_type_serialization() {
        let types = vec![
            NodeType::Blob,
            NodeType::Commit,
            NodeType::Tree,
        ];

        for type_ in types {
            // - Match so that if new variants are introduced, compilation will
            //   fail for not being exhaustive
            let expected = match type_ {
                NodeType::Blob => quote("blob"),
                NodeType::Commit => quote("commit"),
                NodeType::Tree => quote("tree"),
            };

            let actual = serde_json::to_string(&type_).unwrap();

            assert_eq_deserialized(&actual, &expected);
        }
    }

    #[test]
    fn create_a_tree_serialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree
        let expected = r#"{"base_tree":"9fb037999f264ba9a7fc6274d15fa3ae2ab98312","tree":[{"path":"file.rb","mode":"100644","type":"blob","sha":"44b4fc6d56897b048c772eb4087f854f46256132"}]}"#;

        let actual_payload  = RequestBody {
            base_tree: "9fb037999f264ba9a7fc6274d15fa3ae2ab98312".to_owned(),
            tree: vec![
                TreeNode {
                    path: "file.rb".to_owned(),
                    file_mode: FileMode::Blob,
                    node_type: NodeType::Blob,
                    sha_or_content: ShaOrContent::Sha(Some("44b4fc6d56897b048c772eb4087f854f46256132".to_owned())),
                }
            ],
        };

        let actual = serde_json::to_string(&actual_payload).unwrap();

        assert_eq_deserialized(&actual, expected);
    }
}

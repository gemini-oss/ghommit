use std::time::{SystemTime, Duration, UNIX_EPOCH};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use reqwest::blocking::Response;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::github::rest_api::create_an_installation_access_token;
use crate::log::{print_intent, print_intent_plain, print_success_and_return, print_success_plain};

use self::rest_api::{create_a_blob, create_a_commit, create_a_reference, create_a_tree, get_a_reference, update_a_reference};

struct AccessToken {
    token: Arc<String>,
    expires_at: DateTime<Utc>,
}

impl AccessToken {
    fn expires_soon(&self) -> bool {
        let two_minutes_from_now = Utc::now() + chrono::Duration::minutes(2);

        self.expires_at < two_minutes_from_now
    }
}

pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
}

pub struct GitHubClient {
    github_api_base_url: String,
    github_app_id: u64,
    github_app_installation_id: u64,
    github_app_private_key: EncodingKey,
    github_access_token: Mutex<Option<AccessToken>>,
    github_repo: GitHubRepo,
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

enum AuthorizationTokenType {
    AccessToken,
    Jwt,
}

impl GitHubClient {
    pub fn new(github_app_id: u64, github_app_installation_id: u64, github_app_private_key: EncodingKey, github_repo: GitHubRepo) -> GitHubClient {
        GitHubClient {
            github_api_base_url: "https://api.github.com".to_owned(),
            github_app_id: github_app_id,
            github_app_installation_id: github_app_installation_id,
            github_app_private_key: github_app_private_key,
            github_access_token: Mutex::new(None),
            github_repo: github_repo,
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

    /// [Generating a JSON Web Token (JWT) for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app)
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

    pub fn get_access_token(&self, force_token_renewal: bool) -> Result<Arc<String>, String> {
        match self.github_access_token.lock() {
            Ok(mut access_token_guard) => {
                let should_update = force_token_renewal || match &*access_token_guard {
                    Some(token) => token.expires_soon(),
                    None => true,
                };

                if should_update {
                    let raw_access_token = self.create_an_installation_access_token()?;

                    let access_token = AccessToken {
                        token: Arc::new(raw_access_token.token),
                        expires_at: raw_access_token.expires_at,
                    };

                    let return_token = Arc::clone(&access_token.token);

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
    fn base_headers(&self, auth_token_type: AuthorizationTokenType) -> Result<HeaderMap, String> {
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

        let mut headers = HeaderMap::new();

        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
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
        headers.insert(
            &*custom_header::X_GITHUB_API_VERSION,
            HeaderValue::from_static("2022-11-28"),
        );

        Ok(headers)
    }

    fn make_api_request<T: Serialize + ?Sized>(&self, http_method: reqwest::Method, path: &str, json: Option<&T>, auth_token_type: Option<AuthorizationTokenType>) -> Result<Response, String> {
        let url = format!("{}{}", self.github_api_base_url, path);

        let auth_token_type = match auth_token_type {
            Some(auth_token_type) => auth_token_type,
            None => AuthorizationTokenType::AccessToken,
        };

        let http_client = self.get_http_client(None)?;
        let headers = self.base_headers(auth_token_type)?;
        let request = http_client.request(http_method, url).headers(headers);

        let request = match json {
            Some(json) => request.json(&json),
            None => request
        };

        match request.send() {
            Ok(response) => Ok(response),
            Err(e) => Err(format!("Request failed: {}", e)),
        }
    }

    fn get_api_request(&self, path: &str, auth_token_type: Option<AuthorizationTokenType>) -> Result<Response, String> {
        // - The unit type turbofish is necessary to satisfy the type checker
        self.make_api_request::<()>(reqwest::Method::GET, path, None, auth_token_type)
    }

    fn post_api_request<T: Serialize + ?Sized>(&self, path: &str, json: Option<&T>, auth_token_type: Option<AuthorizationTokenType>) -> Result<Response, String> {
        self.make_api_request(reqwest::Method::POST, path, json, auth_token_type)
    }

    fn patch_api_request<T: Serialize + ?Sized>(&self, path: &str, json: Option<&T>, auth_token_type: Option<AuthorizationTokenType>) -> Result<Response, String> {
        self.make_api_request(reqwest::Method::PATCH, path, json, auth_token_type)
    }

    fn unexpected_status_code_error_message(response: Response, operation: &str) -> String {
        let status_code = response.status();

        match &response.text() {
            Ok(text) => format!("Unexpected status code {} while {}: {}", status_code, operation, text),
            Err(_) => format!("Unexpected status code {} while {}; body could not be decoded as text", status_code, operation),
        }
    }

    fn deserialize_expected_response<R: DeserializeOwned>(response: Response, expected_status_code: &StatusCode, operation: &str) -> Result<R, String> {
        let status_code = response.status();

        if &status_code != expected_status_code {
            return Err(Self::unexpected_status_code_error_message(response, operation))
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

    /// [Create a blob](https://docs.github.com/en/rest/git/blobs?apiVersion=2022-11-28#create-a-blob)
    pub fn create_a_blob(&self, payload: &create_a_blob::RequestBody) -> Result<create_a_blob::ResponseBody, String> {
        print_intent("Creating a blob", &payload);

        let path = format!("/repos/{}/{}/git/blobs", self.github_repo.owner, self.github_repo.name);
        let response = self.post_api_request(&path, Some(&payload), None)?;
        let ret = Self::deserialize_expected_response(response, &StatusCode::CREATED, "create a blob");

        print_success_and_return("Blob created", ret)
    }

    /// [Create a tree](https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree)
    pub fn create_a_tree(&self, payload: &create_a_tree::RequestBody) -> Result<create_a_tree::ResponseBody, String> {
        print_intent("Creating a tree", &payload);

        let path = format!("/repos/{}/{}/git/trees", self.github_repo.owner, self.github_repo.name);
        let response = self.post_api_request(&path, Some(&payload), None)?;
        let ret = Self::deserialize_expected_response(response, &StatusCode::CREATED, "create a tree");

        print_success_and_return("Tree created", ret)
    }

    /// [Create a commit](https://docs.github.com/en/rest/git/commits?apiVersion=2022-11-28#create-a-commit)
    pub fn create_a_commit(&self, payload: &create_a_commit::RequestBody) -> Result<create_a_commit::ResponseBody, String> {
        print_intent("Creating a commit", &payload);

        let path = format!("/repos/{}/{}/git/commits", self.github_repo.owner, self.github_repo.name);
        let response = self.post_api_request(&path, Some(&payload), None)?;
        let ret = Self::deserialize_expected_response(response, &StatusCode::CREATED, "create a commit");

        print_success_and_return("Commit created", ret)
    }

    /// [Create a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#create-a-reference)
    pub fn create_a_reference(&self, payload: &create_a_reference::RequestBody) -> Result<create_a_reference::ResponseBody, String> {
        print_intent("Creating reference", &payload);

        let path = format!("/repos/{}/{}/git/refs", self.github_repo.owner, self.github_repo.name);
        let response = self.post_api_request(&path, Some(&payload), None)?;
        let ret = Self::deserialize_expected_response(response, &StatusCode::CREATED, "create a reference");

        print_success_and_return("Reference created", ret)
    }

    /// [Create an installation access token for an app](https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app)
    pub fn create_an_installation_access_token(&self) -> Result<create_an_installation_access_token::ResponseBody, String> {
        print_intent_plain("Creating an installation access token");

        let path = format!("/app/installations/{}/access_tokens", self.github_app_installation_id);
        let response = self.post_api_request::<()>(&path, None, Some(AuthorizationTokenType::Jwt))?;
        let ret = Self::deserialize_expected_response(response, &StatusCode::CREATED, "acquire an access token");

        print_success_plain("Created an installation access token");
        ret
    }

    /// [Get a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#get-a-reference)
    pub fn get_a_reference(&self, partially_qualified_reference_name: &str) -> Result<get_a_reference::ResponseBody, String> {
        print_intent("Getting a reference", &partially_qualified_reference_name);

        let path = format!("/repos/{}/{}/git/refs/{}", self.github_repo.owner, self.github_repo.name, partially_qualified_reference_name);
        let response = self.get_api_request(&path, None)?;

        let operation = "get a reference";

        let status_code = response.status();

        match status_code {
            StatusCode::OK => {
                let success_body = Self::deserialize_expected_response(response, &status_code, operation)?;

                let ret = Ok(get_a_reference::ResponseBody::Ok(success_body));
                print_success_and_return("Reference retrieved", ret)
            },
            StatusCode::NOT_FOUND => {
                let failure_body = Self::deserialize_expected_response(response, &status_code, operation)?;

                let ret = Ok(get_a_reference::ResponseBody::NotFound(failure_body));
                print_success_and_return("Reference retrieved", ret)
            }
            _ => Err(Self::unexpected_status_code_error_message(response, operation)),
        }
    }

    /// [Update a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#update-a-reference)
    pub fn update_a_reference(&self, partially_qualified_reference_name: &str, payload: &update_a_reference::RequestBody) -> Result<update_a_reference::ResponseBody, String> {
        print_intent(&format!("Updating reference {:?}", partially_qualified_reference_name), &payload);

        let path = format!("/repos/{}/{}/git/refs/{}", self.github_repo.owner, self.github_repo.name, partially_qualified_reference_name);
        let response = self.patch_api_request(&path, Some(&payload), None)?;

        let ret = Self::deserialize_expected_response(response, &StatusCode::OK, "update a reference");

        print_success_and_return(&format!("Reference {:?} updated", partially_qualified_reference_name), ret)
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

    /// [Create a commit](https://docs.github.com/en/rest/git/commits?apiVersion=2022-11-28#create-a-commit)
    pub mod create_a_commit {
        use serde::{Deserialize, Serialize};

        /// Abbreviated representation of the response body
        #[derive(Debug, Deserialize, Serialize)]
        pub struct RequestBody {
            pub message: String,
            pub parents: Vec<String>,
            pub tree: String,
        }

        #[derive(Debug, Deserialize, Serialize)]
        pub struct ResponseBody {
            pub sha: String,
            pub html_url: String,
            pub verification: Verification,
        }

        #[derive(Debug, Deserialize, Serialize)]
        pub struct Verification {
            pub verified: bool,
        }
    }


    /// [Create a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#create-a-reference)
    pub mod create_a_reference {
        use serde::{Deserialize, Serialize};

        use super::shared;

        #[derive(Debug, Deserialize, Serialize)]
        pub struct RequestBody {
            #[serde(rename = "ref")]
            pub reference: String,
            pub sha: String,
        }

        pub type ResponseBody = shared::ReferenceResponseBody;
        pub type Object = shared::ReferenceResponseBodyObject;
    }

    /// [Create an installation access token for an app](https://docs.github.com/en/rest/apps/apps?apiVersion=2022-11-28#create-an-installation-access-token-for-an-app)
    pub mod create_an_installation_access_token {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer};

        /// Abbreviated representation of the response body
        #[derive(Debug, Deserialize)]
        pub struct ResponseBody {
            pub token: String,
            #[serde(deserialize_with = "deserialize_datetime")]
            pub expires_at: DateTime<Utc>,
        }

        fn deserialize_datetime<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let s = String::deserialize(deserializer)?;

            DateTime::parse_from_rfc3339(&s)
                .map_err(serde::de::Error::custom)
                .map(|dt| dt.into())
        }
    }

    /// [Create a tree](https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree)
    ///
    /// The entirety of GitHub's trees API uses snake case, so serde
    /// renaming is only necessary for enum variants that derive `Serialize`
    pub mod create_a_tree {
        use serde::{Deserialize, Serialize, Serializer};
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

        /// Abbreviated representation of the response body
        #[derive(Debug, Deserialize, Serialize)]
        pub struct ResponseBody {
            pub sha: String,
            pub truncated: bool,
            pub url: String,
        }
    }

    /// [Get a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#get-a-reference)
    pub mod get_a_reference {
        use serde::{Deserialize, Serialize};

        use super::shared;

        pub type ResponseBodyOk = shared::ReferenceResponseBody;
        pub type Object = shared::ReferenceResponseBodyObject;

        /// Abbreviated representation of the response body
        #[derive(Debug, Deserialize, Serialize)]
        pub struct ResponseBodyNotFound {}

        #[derive(Debug)]
        pub enum ResponseBody {
            Ok(ResponseBodyOk),
            NotFound(ResponseBodyNotFound),
        }
    }

    /// [Update a reference](https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#update-a-reference)
    pub mod update_a_reference {
        use serde::Serialize;

        use super::shared;

        #[derive(Debug, Serialize)]
        pub struct RequestBody {
            pub sha: String,
            pub force: bool,
        }

        pub type ResponseBody = shared::ReferenceResponseBody;
        pub type Object = shared::ReferenceResponseBodyObject;
    }

    pub mod shared {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Deserialize, Serialize)]
        pub struct ReferenceResponseBody {
            #[serde(rename = "ref")]
            pub reference: String,
            pub node_id: String,
            pub url: String,
            pub object: ReferenceResponseBodyObject,
        }

        #[derive(Debug, Deserialize, Serialize)]
        pub struct ReferenceResponseBodyObject {
            #[serde(rename = "type")]
            pub object_type: String,
            pub sha: String,
            pub url: String,
        }
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
mod access_token_tests {
    use std::sync::Arc;

    use chrono::{Utc, Duration};

    use super::AccessToken;

    #[test]
    fn expires_soon() {
        let access_token = AccessToken {
            token: Arc::new("".to_string()),
            expires_at: Utc::now(),
        };

        assert!(access_token.expires_soon())
    }

    #[test]
    fn does_not_expire_soon() {
        let access_token = AccessToken {
            token: Arc::new("".to_string()),
            expires_at: Utc::now() + Duration::minutes(5),
        };

        assert!(!access_token.expires_soon())
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
mod create_a_commit_tests {
    use crate::github::rest_api::create_a_commit::RequestBody;
    use crate::github::test_util::assert_eq_deserialized;

    use super::rest_api::create_a_commit::{ResponseBody, Verification};

    #[test]
    fn create_a_commit_serialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/commits?apiVersion=2022-11-28#create-a-commit
        let original_payload = r#"{"message":"my commit message","author":{"name":"Mona Octocat","email":"octocat@github.com","date":"2008-07-09T16:13:30+12:00"},"parents":["7d1b31e74ee336d15cbd21741bc88a537ed063a0"],"tree":"827efc6d56897b048c772eb4087f854f46256132","signature":"-----BEGIN PGP SIGNATURE-----\n\niQIzBAABAQAdFiEESn/54jMNIrGSE6Tp6cQjvhfv7nAFAlnT71cACgkQ6cQjvhfv\n7nCWwA//XVqBKWO0zF+bZl6pggvky3Oc2j1pNFuRWZ29LXpNuD5WUGXGG209B0hI\nDkmcGk19ZKUTnEUJV2Xd0R7AW01S/YSub7OYcgBkI7qUE13FVHN5ln1KvH2all2n\n2+JCV1HcJLEoTjqIFZSSu/sMdhkLQ9/NsmMAzpf/iIM0nQOyU4YRex9eD1bYj6nA\nOQPIDdAuaTQj1gFPHYLzM4zJnCqGdRlg0sOM/zC5apBNzIwlgREatOYQSCfCKV7k\nnrU34X8b9BzQaUx48Qa+Dmfn5KQ8dl27RNeWAqlkuWyv3pUauH9UeYW+KyuJeMkU\n+NyHgAsWFaCFl23kCHThbLStMZOYEnGagrd0hnm1TPS4GJkV4wfYMwnI4KuSlHKB\njHl3Js9vNzEUQipQJbgCgTiWvRJoK3ENwBTMVkKHaqT4x9U4Jk/XZB6Q8MA09ezJ\n3QgiTjTAGcum9E9QiJqMYdWQPWkaBIRRz5cET6HPB48YNXAAUsfmuYsGrnVLYbG+\nUpC6I97VybYHTy2O9XSGoaLeMI9CsFn38ycAxxbWagk5mhclNTP5mezIq6wKSwmr\nX11FW3n1J23fWZn5HJMBsRnUCgzqzX3871IqLYHqRJ/bpZ4h20RhTyPj5c/z7QXp\neSakNQMfbbMcljkha+ZMuVQX1K9aRlVqbmv3ZMWh+OijLYVU2bc=\n=5Io4\n-----END PGP SIGNATURE-----\n"}"#;

        let actual = {
            // - Deserialize and reserialize since `RequestBody` is an
            //   abbreviated representation
            let actual_deserialized = serde_json::from_str::<RequestBody>(&original_payload).unwrap();

            serde_json::to_string(&actual_deserialized).unwrap()
        };

        let expected = {
            let expected_deserialized = RequestBody {
                message: "my commit message".to_string(),
                parents: vec!["7d1b31e74ee336d15cbd21741bc88a537ed063a0".to_string()],
                tree: "827efc6d56897b048c772eb4087f854f46256132".to_string(),
            };

            serde_json::to_string(&expected_deserialized).unwrap()
        };

        assert_eq_deserialized(&actual, &expected);
    }

    #[test]
    fn create_a_commit_deserialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/commits?apiVersion=2022-11-28#create-a-commit
        let original_payload = r#"
            {
              "sha": "7638417db6d59f3c431d3e1f261cc637155684cd",
              "node_id": "MDY6Q29tbWl0NzYzODQxN2RiNmQ1OWYzYzQzMWQzZTFmMjYxY2M2MzcxNTU2ODRjZA==",
              "url": "https://api.github.com/repos/octocat/Hello-World/git/commits/7638417db6d59f3c431d3e1f261cc637155684cd",
              "author": {
                "date": "2014-11-07T22:01:45Z",
                "name": "Monalisa Octocat",
                "email": "octocat@github.com"
              },
              "committer": {
                "date": "2014-11-07T22:01:45Z",
                "name": "Monalisa Octocat",
                "email": "octocat@github.com"
              },
              "message": "my commit message",
              "tree": {
                "url": "https://api.github.com/repos/octocat/Hello-World/git/trees/827efc6d56897b048c772eb4087f854f46256132",
                "sha": "827efc6d56897b048c772eb4087f854f46256132"
              },
              "parents": [
                {
                  "url": "https://api.github.com/repos/octocat/Hello-World/git/commits/7d1b31e74ee336d15cbd21741bc88a537ed063a0",
                  "sha": "7d1b31e74ee336d15cbd21741bc88a537ed063a0",
                  "html_url": "https://github.com/octocat/Hello-World/commit/7d1b31e74ee336d15cbd21741bc88a537ed063a0"
                }
              ],
              "verification": {
                "verified": false,
                "reason": "unsigned",
                "signature": null,
                "payload": null
              },
              "html_url": "https://github.com/octocat/Hello-World/commit/7638417db6d59f3c431d3e1f261cc637155684cd"
            }
        "#;

        let actual = {
            let actual_serialized = serde_json::from_str::<ResponseBody>(&original_payload).unwrap();

            serde_json::to_string(&actual_serialized).unwrap()
        };

        let expected = {
            let expected_serialized = ResponseBody {
                sha: "7638417db6d59f3c431d3e1f261cc637155684cd".to_string(),
                html_url: "https://github.com/octocat/Hello-World/commit/7638417db6d59f3c431d3e1f261cc637155684cd".to_string(),
                verification: Verification {
                    verified: false,
                },
            };

            serde_json::to_string(&expected_serialized).unwrap()
        };

        assert_eq_deserialized(&actual, &expected);
    }
}

#[cfg(test)]
mod create_a_reference_tests {
    use crate::github::rest_api::create_a_reference::RequestBody;
    use crate::github::test_util::assert_eq_deserialized;

    #[test]
    fn deserialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#create-a-reference
        let expected = r#"{"ref":"refs/heads/featureA","sha":"aa218f56b14c9653891f9e74264a383fa43fefbd"}"#;

        let actual = {
            let actual_serialized = serde_json::from_str::<RequestBody>(&expected).unwrap();

            serde_json::to_string(&actual_serialized).unwrap()
        };

        assert_eq_deserialized(&actual, expected);
    }
}

#[cfg(test)]
mod create_a_tree_tests {
    use super::rest_api::create_a_tree::{FileMode, NodeType, RequestBody, ResponseBody, ShaOrContent, TreeNode};
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

    #[test]
    fn create_a_tree_deserialization_with_github_example_payload() {
        let actual = {
            // From the docs: https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree
            let original = r#"
                {
                  "sha": "cd8274d15fa3ae2ab983129fb037999f264ba9a7",
                  "url": "https://api.github.com/repos/octocat/Hello-World/trees/cd8274d15fa3ae2ab983129fb037999f264ba9a7",
                  "tree": [
                    {
                      "path": "file.rb",
                      "mode": "100644",
                      "type": "blob",
                      "size": 132,
                      "sha": "7c258a9869f33c1e1e1f74fbb32f07c86cb5a75b",
                      "url": "https://api.github.com/repos/octocat/Hello-World/git/blobs/7c258a9869f33c1e1e1f74fbb32f07c86cb5a75b"
                    }
                  ],
                  "truncated": true
                }
            "#;

            let actual_deserialized = serde_json::from_str::<ResponseBody>(&original).unwrap();

            serde_json::to_string(&actual_deserialized).unwrap()
        };

        let expected = {
            let expected_deserialized = ResponseBody {
                sha: "cd8274d15fa3ae2ab983129fb037999f264ba9a7".to_string(),
                truncated: true,
                url: "https://api.github.com/repos/octocat/Hello-World/trees/cd8274d15fa3ae2ab983129fb037999f264ba9a7".to_string(),
            };

            serde_json::to_string(&expected_deserialized).unwrap()
        };

        assert_eq_deserialized(&actual, &expected);
    }
}

#[cfg(test)]
mod get_a_reference_tests {
    use super::rest_api::get_a_reference::ResponseBodyNotFound;
    use super::test_util::assert_eq_deserialized;

    #[test]
    fn not_found_deserialization() {
        let actual = {
            let original = r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest/reference/git#get-a-reference"}"#;

            let actual_deserialized = serde_json::from_str::<ResponseBodyNotFound>(&original).unwrap();

            serde_json::to_string(&actual_deserialized).unwrap()
        };

        let expected = {
            let expected_deserialized = ResponseBodyNotFound {};

            serde_json::to_string(&expected_deserialized).unwrap()
        };

        assert_eq_deserialized(&actual, &expected);
    }
}

#[cfg(test)]
mod update_a_reference_tests {
    use super::rest_api::update_a_reference::RequestBody;
    use super::test_util::assert_eq_deserialized;

    #[test]
    fn update_a_reference_serialization_with_github_example_payload() {
        // From the docs: https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#update-a-reference
        let expected = r#"{"sha":"aa218f56b14c9653891f9e74264a383fa43fefbd","force":true}"#;

        let actual = {
            let actual_deserialized = RequestBody {
                sha: "aa218f56b14c9653891f9e74264a383fa43fefbd".to_string(),
                force: true,
            };

            serde_json::to_string(&actual_deserialized).unwrap()
        };

        assert_eq_deserialized(&actual, expected);
    }
}

#[cfg(test)]
mod shared_tests {
    use super::rest_api::shared::{ReferenceResponseBody, ReferenceResponseBodyObject};
    use super::test_util::assert_eq_deserialized;

    #[test]
    fn reference_deserialization_with_github_example_payload() {
        // From the docs:
        // - https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#get-a-reference
        // - https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#create-a-reference
        // - https://docs.github.com/en/rest/git/refs?apiVersion=2022-11-28#update-a-reference
        let expected = r#"
            {
              "ref": "refs/heads/featureA",
              "node_id": "MDM6UmVmcmVmcy9oZWFkcy9mZWF0dXJlQQ==",
              "url": "https://api.github.com/repos/octocat/Hello-World/git/refs/heads/featureA",
              "object": {
                "type": "commit",
                "sha": "aa218f56b14c9653891f9e74264a383fa43fefbd",
                "url": "https://api.github.com/repos/octocat/Hello-World/git/commits/aa218f56b14c9653891f9e74264a383fa43fefbd"
              }
            }
        "#;

        let actual = {
            let actual_deserialized = ReferenceResponseBody {
                reference: "refs/heads/featureA".to_string(),
                node_id: "MDM6UmVmcmVmcy9oZWFkcy9mZWF0dXJlQQ==".to_string(),
                url: "https://api.github.com/repos/octocat/Hello-World/git/refs/heads/featureA".to_string(),
                object: ReferenceResponseBodyObject {
                    object_type: "commit".to_string(),
                    sha: "aa218f56b14c9653891f9e74264a383fa43fefbd".to_string(),
                    url: "https://api.github.com/repos/octocat/Hello-World/git/commits/aa218f56b14c9653891f9e74264a383fa43fefbd".to_string(),
                },
            };

            serde_json::to_string(&actual_deserialized).unwrap()
        };

        assert_eq_deserialized(&actual, expected);
    }
}

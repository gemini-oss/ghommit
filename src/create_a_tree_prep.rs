use std::collections::HashSet;
use std::io::Write;

use base64::write::EncoderStringWriter;
use once_cell::sync::Lazy;

use crate::config::Config;
use crate::github::GitHubClient;
use crate::github::rest_api::create_a_blob;
use crate::github::rest_api::create_a_tree;
use crate::git_status::PathStatus;

/// - [GitHub's Create a tree API endpoint](https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree)
/// requires a `tree`'s `mode` property to be one of their accepted values when
/// deleting a file
/// - This doesn't make sense since a deleted file has no mode
///   - For reference, `git2` reports deleted files' mode as
///   `git2::FileMode::Unreadable`
/// - Upon testing, the API responds the same no matter what accepted mode is
/// used, so `Blob` was chosen arbitrarily
const DELETED_FILE_MODE: create_a_tree::FileMode = create_a_tree::FileMode::Blob;

/// - [GitHub's Create a tree API endpoint](https://docs.github.com/en/rest/git/trees?apiVersion=2022-11-28#create-a-tree)
/// _does not_ require a node type to be present for deleted files
/// - The presence of a node type doesn't affect the behavior
/// - Since it doesn't change the behavior and since all other non-deletion
/// operations require a node type, we pass a benign value rather than resorting
/// to `Option<NodeType>` as it would introduce invalid states
/// - A "more correct" fix is possible, but the cost of the complexity does not
/// seem worthwhile
const DELETED_NODE_TYPE: create_a_tree::NodeType = create_a_tree::NodeType::Blob;

#[derive(Debug)]
enum ObjectContents {
    Text(String),
    Base64(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum GitCommitAction {
    AddPath,
    DeleteOriginalPath,
    DeletePath,
    Nop,
    Unsupported,
}

fn delta_to_actions(git2_delta: git2::Delta) -> &'static HashSet<GitCommitAction> {
    static ADD_PATH: Lazy<HashSet<GitCommitAction>> = Lazy::new(|| HashSet::from([
        GitCommitAction::AddPath,
    ]));
    static ADD_PATH_AND_DELETE_ORIGINAL_PATH: Lazy<HashSet<GitCommitAction>> = Lazy::new(|| HashSet::from([
        GitCommitAction::AddPath,
        GitCommitAction::DeleteOriginalPath,
    ]));
    static DELETE_PATH: Lazy<HashSet<GitCommitAction>> = Lazy::new(|| HashSet::from([
        GitCommitAction::DeletePath,
    ]));
    static NOP: Lazy<HashSet<GitCommitAction>> = Lazy::new(|| HashSet::from([
        GitCommitAction::Nop,
    ]));
    static UNSUPPORTED: Lazy<HashSet<GitCommitAction>> = Lazy::new(|| HashSet::from([
        GitCommitAction::Unsupported,
    ]));

    match git2_delta {
        // Add path
        git2::Delta::Modified => &ADD_PATH,
        git2::Delta::Added => &ADD_PATH,
        git2::Delta::Copied => &ADD_PATH,
        git2::Delta::Typechange => &ADD_PATH,
        // Add path and delete original path
        git2::Delta::Renamed => &ADD_PATH_AND_DELETE_ORIGINAL_PATH,
        // Delete path
        git2::Delta::Deleted => &DELETE_PATH,
        // // NOPs
        git2::Delta::Unmodified => &NOP,
        git2::Delta::Ignored => &NOP,
        git2::Delta::Untracked => &NOP,
        // Unsupported
        git2::Delta::Unreadable => &UNSUPPORTED,
        git2::Delta::Conflicted => &UNSUPPORTED,
    }
}

/// Note: Files must be read by their object ID, not the path, since
/// modifications could have been made after being staged
fn read_file(path_status: &PathStatus, git_object: &git2::Object) -> Result<ObjectContents, String> {
    if let Some(blob) = git_object.as_blob() {
        let bytes = blob.content();

        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(ObjectContents::Text(text.to_string())),
            Err(_) => {
                let mut enc = EncoderStringWriter::new(&base64::engine::general_purpose::STANDARD);

                match enc.write_all(bytes) {
                    Ok(_) => Ok(ObjectContents::Base64(enc.into_inner())),
                    Err(_) => Err(format!("Failed to Base64-encode contents of path {} with corresponding object ID {:?}", path_status.path, git_object.id())),
                }
            }
        }
    } else {
        Err(format!("Path {:?} was expected to be a blob, but found {:?}", path_status.path, git_object.kind()))
    }
}

fn git2_mode_to_github_mode(path_status: &PathStatus) -> Result<create_a_tree::FileMode, String> {
    let github_mode = match path_status.file_mode {
        git2::FileMode::Blob => create_a_tree::FileMode::Blob,
        git2::FileMode::BlobExecutable => create_a_tree::FileMode::BlobExecutable,
        git2::FileMode::Commit => create_a_tree::FileMode::Commit,
        git2::FileMode::Link => create_a_tree::FileMode::Link,
        git2::FileMode::Tree => create_a_tree::FileMode::Tree,

        // - Not supported by GitHub
        // git2::FileMode::BlobGroupWritable => todo!(),
        // git2::FileMode::Unreadable => todo!(),
        _ => Err(format!("Path {:?} has file mode {:?} which is not supported by GitHub", path_status.path, path_status.file_mode))?,
    };

    Ok(github_mode)
}

fn git2_node_type_to_github_node_type(path_status: &PathStatus, git_object: &git2::Object) -> Result<create_a_tree::NodeType, String> {
    match git_object.kind() {
        Some(object_type) => {
            let node_type = match object_type {
                git2::ObjectType::Blob => create_a_tree::NodeType::Blob,
                git2::ObjectType::Commit => create_a_tree::NodeType::Commit,
                git2::ObjectType::Tree => create_a_tree::NodeType::Tree,

                // - Not supported by GitHub
                // git2::ObjectType::Any => todo!(),
                // git2::ObjectType::Tag => todo!(),
                _ => Err(format!("Path {:?} has object type {:?} which is not supported by GitHub's API", path_status.path, git_object.kind()))?
            };

            Ok(node_type)
        },
        None => Err(format!("Unknown object type on object with ID {:?} with path {:?}", git_object.id(), path_status.path)),
    }
}

pub fn generate_request_body(config: &Config, repo: &git2::Repository, git_status: &Vec<PathStatus>, github_client: &GitHubClient) -> Result<create_a_tree::RequestBody, String> {
    let mut tree = Vec::with_capacity(git_status.len());

    for path_status in git_status {
        let actions = delta_to_actions(path_status.delta);

        let git_object_id = path_status.object_id;
        let git_object = repo.find_object(git_object_id, None)
            .map_err(|_| format!("Unable to find object {:?} in repo {:?}", git_object_id, repo.path()))?;

        for action in actions {
            let path = path_status.path.clone();
            let node_type = git2_node_type_to_github_node_type(path_status, &git_object)?;

            match action {
                GitCommitAction::AddPath => {
                    let file_mode = git2_mode_to_github_mode(path_status)?;
                    let object_contents = read_file(path_status, &git_object)?;

                    let sha_or_content = match object_contents {
                        ObjectContents::Text(text) => create_a_tree::ShaOrContent::Content(text),
                        ObjectContents::Base64(base64_string) => {
                            let request_body = create_a_blob::RequestBody {
                                content: &base64_string,
                                encoding: create_a_blob::Encoding::Base64,
                            };

                            let response = github_client.create_a_blob(config, &request_body)?;
                            create_a_tree::ShaOrContent::Sha(Some(response.sha))
                        },
                    };

                    let node = create_a_tree::TreeNode {
                        path: path,
                        file_mode: file_mode,
                        node_type: node_type,
                        sha_or_content: sha_or_content,
                    };

                    tree.push(node);
                },
                GitCommitAction::DeletePath | GitCommitAction::DeleteOriginalPath => {
                    let path = match action {
                        GitCommitAction::DeletePath => path,
                        GitCommitAction::DeleteOriginalPath => {
                            match &path_status.original_path {
                                Some(path) => path.clone(),
                                None => Err(format!("Expected an original path, but none was found for {:?}", path_status))?,
                            }
                        },
                        _ => Err(format!("Expected delete action, but found {:?}", action))?,
                    };

                    let node = create_a_tree::TreeNode {
                        path: path,
                        file_mode: DELETED_FILE_MODE,
                        node_type: DELETED_NODE_TYPE,
                        sha_or_content: create_a_tree::ShaOrContent::Sha(None),
                    };

                    tree.push(node);
                },
                GitCommitAction::Nop => {},
                GitCommitAction::Unsupported => {
                    Err(format!("Unsupported delta {:?} for path status {:?}", path_status.delta, path_status))?
                },
            }
        }
    }

    let body = create_a_tree::RequestBody {
        base_tree: config.git_head_object_id.clone(),
        tree: tree,
    };

    Ok(body)
}

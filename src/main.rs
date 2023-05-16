#![allow(clippy::redundant_field_names)]

use std::collections::HashSet;
use std::io::Write;

use base64::write::EncoderStringWriter;
use once_cell::sync::Lazy;

use ghommit::config::Config;
use ghommit::git_status::{git_status, PathStatus};
use ghommit::github::GitHubClient;
use ghommit::github::request::{CreateCommitOnBranchInput, CommittableBranch, CommitMessage, FileAddition, FileChanges, FileDeletion};

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

fn read_as_base64(repo: &git2::Repository, object_id: git2::Oid) -> Result<String, String> {
    let object = repo.find_object(object_id, None)
        .map_err(|_| format!("Unable to find object {:?} in repo {:?}", object_id, repo.path()))?;

    if let Some(blob) = object.as_blob() {
        let bytes = blob.content();

        let mut enc = EncoderStringWriter::new(&base64::engine::general_purpose::STANDARD);

        match enc.write_all(bytes) {
            Ok(_) => Ok(enc.into_inner()),
            Err(_) => Err(format!("Failed to Base64-encode contents of object {:?} in repo {:?}", object_id, repo.path())),
        }
    } else {
        Err(format!("Expected object {:?} in repo {:?} to be a blob, but found {:?}", object_id, repo.path(), object.kind()))
    }
}

fn path_statuses_to_file_changes(repo: &git2::Repository, status: &Vec<PathStatus>) -> Result<FileChanges, String> {
    fn checks(additions: &Vec<FileAddition>, deletions: &Vec<FileDeletion>) -> Result<(), String> {
        // Emptiness
        if additions.is_empty() && deletions.is_empty() {
            Err("No changes to commit".to_owned())?
        }

        // Duplicates
        let additions_set: HashSet<_> = additions.iter().map(|a| &a.path).collect();
        let deletions_set: HashSet<_> = deletions.iter().map(|d| &d.path).collect();

        if additions_set.len() != additions.len() {
            Err(format!("Files were added more than once: {:?}", additions))?
        }

        if deletions_set.len() != deletions.len() {
            Err(format!("Files were deleted more than once: {:?}", deletions))?
        }

        // Files both added and deleted
        let intersection: HashSet<_> = additions_set.intersection(&deletions_set).collect();

        if !intersection.is_empty() {
            Err(format!("Some files were added and deleted: {:?}", intersection))?
        }

        Ok(())
    }

    let mut additions = vec![];
    let mut deletions = vec![];

    for path_status in status {
        let actions = delta_to_actions(path_status.delta);

        for action in actions {
            match action {
                GitCommitAction::AddPath => {
                    let path = path_status.path.clone();

                    let file_addition = FileAddition {
                        contents: read_as_base64(repo, path_status.object_id)?,
                        path: path,
                    };

                    additions.push(file_addition)
                }
                GitCommitAction::DeleteOriginalPath => {
                    let path = match &path_status.original_path {
                        Some(path) => path.clone(),
                        None => Err(format!("Expected an original path, but none was found for {:?}", path_status))?,
                    };

                    let file_deletion = FileDeletion {
                        path: path,
                    };

                    deletions.push(file_deletion);
                },
                GitCommitAction::DeletePath => {
                    let file_deletion = FileDeletion {
                        path: path_status.path.clone(),
                    };

                    deletions.push(file_deletion);
                },
                GitCommitAction::Nop => {},
                GitCommitAction::Unsupported => {
                    Err(format!("Unsupported delta {:?} for path status {:?}", path_status.delta, path_status))?
                },
            }
        }
    }

    checks(&additions, &deletions)?;

    Ok(FileChanges {
        additions: additions,
        deletions: deletions,
    })
}

/// Returns a URL to the commit on GitHub.
fn commit_via_github_api(github_client: &GitHubClient, config: &Config, file_changes: FileChanges) -> Result<String, String> {
    let repo_owner = &config.github_repo_owner;
    let repo_name = &config.github_repo_name;

    let repo_owner_and_name = format!("{}/{}", repo_owner, repo_name);

    let args = CreateCommitOnBranchInput {
        branch: CommittableBranch {
            repository_name_with_owner: repo_owner_and_name,
            branch_name: config.git_branch_name.clone(),
        },
        client_mutation_id: None,
        expected_head_oid: config.git_head_object_id.clone(),
        file_changes: Some(file_changes),
        message: CommitMessage {
            headline: config.commit_message.clone(),
            body: None,
        }
    };

    github_client.create_commit_on_branch(config, args)
}

fn ghommit() -> Result<String, String> {
    let maybe_repo = git2::Repository::open(".");
    let config = Config::gather(maybe_repo)?;

    let status = git_status(&config.git_repo)?;

    let file_changes = path_statuses_to_file_changes(&config.git_repo, &status)?;

    let github_client = GitHubClient::new(
        config.github_app_id,
        config.github_app_installation_id,
        config.github_app_private_key.clone(),
    );

    let commit_url = commit_via_github_api(&github_client, &config, file_changes)?;

    Ok(commit_url)
}

fn main() -> Result<(), String> {
    // Match so that Strings in an Err can be pulled out and printed without
    // the Err wrapping so newlines aren't escaped
    match ghommit() {
        Ok(commit_url) => {
            println!("Commit created: {}", commit_url);
            Ok(())
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1)
        }
    }
}

use std::collections::HashSet;
use std::fs;
use std::io::Write;

use base64;
use base64::write::EncoderStringWriter;
use git2;

use crate::config::Config;
use crate::git_status::{git_status, PathStatus};
use crate::github::GitHubClient;
use crate::github::request::{CreateCommitOnBranchInput, CommittableBranch, CommitMessage, FileAddition, FileChanges, FileDeletion};

mod config;
mod github;
mod git_status;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum GitCommitAction {
    AddPath,
    DeleteOriginalPath,
    DeletePath,
    NOP,
    Unsupported,
}

fn delta_to_actions(git2_delta: git2::Delta) -> HashSet<GitCommitAction> {
    fn set(actions: &[GitCommitAction]) -> HashSet<GitCommitAction> {
        actions.iter().cloned().collect()
    }

    match git2_delta {
        // Add path
        git2::Delta::Modified => set(&[GitCommitAction::AddPath]),
        git2::Delta::Added => set(&[GitCommitAction::AddPath]),
        git2::Delta::Copied => set(&[GitCommitAction::AddPath]),
        git2::Delta::Typechange => set(&[GitCommitAction::AddPath]),
        // Add path and delete original path
        git2::Delta::Renamed => set(&[GitCommitAction::AddPath, GitCommitAction::DeleteOriginalPath]),
        // Delete path
        git2::Delta::Deleted => set(&[GitCommitAction::DeletePath]),
        // NOPs
        git2::Delta::Unmodified => set(&[GitCommitAction::NOP]),
        git2::Delta::Ignored => set(&[GitCommitAction::NOP]),
        git2::Delta::Untracked => set(&[GitCommitAction::NOP]),
        // Unsupported
        git2::Delta::Unreadable => set(&[GitCommitAction::Unsupported]),
        git2::Delta::Conflicted => set(&[GitCommitAction::Unsupported]),
    }
}

fn read_as_base64(path: &str) -> Result<String, String> {
    let mut enc = EncoderStringWriter::new(&base64::engine::general_purpose::STANDARD);

    match fs::read(path) {
        Ok(buf) => {
            match enc.write_all(&buf) {
                Ok(_) => Ok(enc.into_inner()),
                Err(_) => Err(format!("Failed to Base64-encode path {}", path)),
            }
        },
        Err(_) => Err(format!("Unable to read path {}", path)),
    }
}

fn path_statuses_to_file_changes(status: &Vec<PathStatus>) -> Result<FileChanges, String> {
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

        if intersection.len() > 0 {
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
                        contents: read_as_base64(&path)?,
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
                GitCommitAction::NOP => {},
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
    let config = config::Config::gather_config(maybe_repo)?;

    let status = git_status(&config)?;

    let file_changes = path_statuses_to_file_changes(&status)?;

    let github_client = GitHubClient{
        github_app_id: config.github_app_id,
        github_app_installation_id: config.github_app_installation_id,
        github_app_private_key: &config.github_app_private_key,
    };

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

#![allow(clippy::redundant_field_names)]

use ghommit::config::Config;
use ghommit::create_a_tree_prep;
use ghommit::git_status::git_status;
use ghommit::github::{GitHubClient, GitHubRepo};
use ghommit::github::rest_api::{create_a_commit, create_a_reference, get_a_reference, update_a_reference};

fn create_github_client(config: &Config) -> GitHubClient {
    let github_repo = GitHubRepo {
        owner: config.github_repo_owner.clone(),
        name: config.github_repo_name.clone(),
    };

    GitHubClient::new(
        config.github_app_id,
        config.github_app_installation_id,
        config.github_app_private_key.clone(),
        github_repo,
    )
}

fn generate_create_a_commit_body(config: &Config, tree_sha: &str) -> create_a_commit::RequestBody {
    create_a_commit::RequestBody {
        message: config.commit_message.to_string(),
        parents: vec![config.git_head_object_id.to_string()],
        tree: tree_sha.to_string(),
    }
}

fn fully_qualify_branch_name(unqualified_name: &str) -> String {
    format!("refs/heads/{}", unqualified_name)
}

fn partially_qualify_branch_name(unqualified_name: &str) -> String {
    format!("heads/{}", unqualified_name)
}

fn branch_exists(github_client: &GitHubClient, config: &Config) -> Result<bool, String> {
    let reference_name = partially_qualify_branch_name(&config.git_branch_name);

    let get_a_reference_response = github_client.get_a_reference(&reference_name);

    let exists = match get_a_reference_response {
        Ok(get_a_reference_response) => {
            match get_a_reference_response {
                get_a_reference::ResponseBody::Ok(_) => true,
                get_a_reference::ResponseBody::NotFound(_) => false,
            }
        },
        Err(err_string) => Err(err_string)?,
    };

    Ok(exists)
}

fn update_a_reference(config: &Config, github_client: &GitHubClient, commit_sha: &str) -> Result<update_a_reference::ResponseBody, String> {
    let payload = update_a_reference::RequestBody {
        sha: commit_sha.to_string(),
        force: config.git_should_force_push,
    };

    let reference_name = partially_qualify_branch_name(&config.git_branch_name);

    github_client.update_a_reference(&reference_name, &payload)
}

fn create_a_reference(config: &Config, github_client: &GitHubClient, commit_sha: &str) -> Result<create_a_reference::ResponseBody, String> {
    let payload = create_a_reference::RequestBody {
        reference: fully_qualify_branch_name(&config.git_branch_name),
        sha: commit_sha.to_string(),
    };

    github_client.create_a_reference(&payload)
}

fn ghommit() -> Result<String, String> {
    let maybe_repo = git2::Repository::open(".");
    let config = Config::gather(maybe_repo)?;

    let status = git_status(&config.git_repo)?;

    if status.is_empty() {
        return Err("No changes to commit".to_string())
    }

    let github_client = create_github_client(&config);

    // - Create the tree, creating the blobs if necessary implicitly

    let tree_payload = create_a_tree_prep::generate_request_body(&config, &config.git_repo, &status, &github_client)?;
    let tree = github_client.create_a_tree(&tree_payload)?;

    // - Create the commit

    let commit_payload = generate_create_a_commit_body(&config, &tree.sha);
    let commit = github_client.create_a_commit(&commit_payload)?;

    // - If branch exists, update it, else create it

    match branch_exists(&github_client, &config)? {
        true => update_a_reference(&config, &github_client, &commit.sha),
        false => create_a_reference(&config, &github_client, &commit.sha),
    }?;

    Ok(commit.html_url)
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

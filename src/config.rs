use std::{env, fmt};

use clap::Parser;
use git2::{Repository, Error};
use jsonwebtoken::EncodingKey;

pub struct Config {
    pub commit_message: String,
    pub git_branch_name: String,
    pub git_head_object_id: String,
    pub git_repo: Repository,
    pub git_should_force_push: bool,
    pub github_app_id: u64,
    pub github_app_installation_id: u64,
    pub github_app_private_key: EncodingKey,
    pub github_repo_owner: String,
    pub github_repo_name: String,
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Config {{ ")?;
        write!(f, "commit_message: \"{}\"", self.commit_message)?;
        write!(f, ", git_branch: \"{}\"", self.git_branch_name)?;
        write!(f, ", git_head_object_id: \"{}\"", self.git_head_object_id)?;
        write!(f, ", git_repo: Repository {{ {} }}", self.git_repo.path().to_str().unwrap_or("(unknown)"))?;
        write!(f, ", git_should_force_push: {}", self.git_should_force_push)?;
        write!(f, ", github_app_id: {}", self.github_app_id)?;
        write!(f, ", github_app_installation_id: {}", self.github_app_installation_id)?;
        write!(f, ", github_app_private_key: EncodingKey {{ ... }} ")?;
        write!(f, ", github_repo_owner: \"{}\"", self.github_repo_owner)?;
        write!(f, ", github_repo_name: \"{}\"", self.github_repo_name)?;
        write!(f, " }}")?;
        Ok(())
    }
}

/// ghommit: GitHub commit
#[derive(Debug)]
#[derive(clap::Parser)]
#[command(name = "ghommit")]
struct CommandLineArgumentsRaw {
    /// GitHub owner and repo name in $OWNER/$REPO_NAME format
    #[arg(long, required = true)]
    github_owner_and_repo: String,

    /// Commit message
    #[arg(long, short, required = true)]
    message: String,

    /// Force push
    #[arg(long, short, default_value = "false")]
    force: bool,
}

#[derive(Debug)]
struct CommandLineArguments {
    commit_message: String,
    git_should_force_push: bool,
    github_repo_owner: String,
    github_repo_name: String,
}

struct GitConfig {
    branch_name: String,
    git_head_object_id: String,
    repository: Repository,
}

impl Config {
    fn command_line_arguments() -> Result<CommandLineArguments, String> {
        let raw_args = match CommandLineArgumentsRaw::try_parse() {
            Ok(res) => res,
            Err(e) => Err(e.to_string())?,
        };

        let repo_and_owner_split: Vec<&str> = raw_args.github_owner_and_repo.split('/').collect();

        match repo_and_owner_split.as_slice() {
            [owner, repo_name] => {
                Ok(CommandLineArguments {
                    commit_message: raw_args.message,
                    git_should_force_push: raw_args.force,
                    github_repo_owner: owner.to_string(),
                    github_repo_name: repo_name.to_string(),
                })
            }
            _ => {
                Err(format!("Expected --github-owner-and-repo in $REPO/$OWNER, but found {}", raw_args.github_owner_and_repo))
            }
        }
    }

    fn environment_variable(name: &str) -> Result<String, String> {
        match env::var(name) {
            Ok(result) => Ok(result),
            Err(_) => Err(format!("Environment variable not set: {}", name)),
        }
    }

    fn environment_variable_rsa_private_key(name: &str) -> Result<EncodingKey, String> {
        let pem_data = Config::environment_variable(name)?;

        match EncodingKey::from_rsa_pem(pem_data.as_bytes()) {
            Ok(key) => Ok(key),
            Err(_) => Err(format!("Environment variable {} is not valid RSA private key", name)),
        }
    }

    fn environment_variable_u64(name: &str) -> Result<u64, String> {
        let as_string = Config::environment_variable(name)?;

        match as_string.parse::<u64>() {
            Ok(result) => Ok(result),
            Err(_) => Err(format!("Environment variable {} cannot be parsed as u64: {}", name, as_string)),
        }
    }

    fn git_config(maybe_repo: Result<Repository, Error>) -> Result<GitConfig, String> {
        match maybe_repo {
            Ok(repo) => {
                let (branch_name, head_object_id) = match repo.head() {
                    Ok(head) => {
                        let branch_name = match head.shorthand() {
                            Some(name) => Ok(name.to_owned()),
                            None => Err("Git repository HEAD name is invalid".to_owned()),
                        }?;

                        let head_object_id = match head.peel_to_commit() {
                            Ok(commit) => Ok(commit.id().to_string()),
                            Err(_) => Err(format!("Could not resolve commit for branch {}", branch_name)),
                        }?;

                        // - Because the repo matched against needs to be
                        //   returned as well, the return cannot be in this
                        //   scope, so bail to outer scope
                        Ok((branch_name, head_object_id))
                    },
                    Err(_) => Err("Git repository doesn't have a HEAD".to_owned()),
                }?;

                Ok(GitConfig {
                    branch_name: branch_name,
                    git_head_object_id: head_object_id,
                    repository: repo,
                })
            },
            Err(_) => Err("Not in a Git repository".to_owned()),
        }
    }

    /// Gathers the config from command line arguments, the Git repository, and
    /// from environment variables.
    pub fn gather_config(maybe_repo: Result<Repository, Error>) -> Result<Config, String> {
        // Command line arguments
        let cli_args = Self::command_line_arguments()?;

        // Git
        let git_config = Self::git_config(maybe_repo)?;

        // Environment variables
        let github_app_id = Self::environment_variable_u64("GHOMMIT_GITHUB_APP_ID")?;
        let github_app_installation_id = Self::environment_variable_u64("GHOMMIT_GITHUB_APP_INSTALLATION_ID")?;
        let github_app_private_key_pem_data = Self::environment_variable_rsa_private_key("GHOMMIT_GITHUB_APP_PRIVATE_KEY_PEM_DATA")?;

        #[allow(clippy::redundant_field_names)]
        Ok(Config {
            commit_message: cli_args.commit_message,
            git_branch_name: git_config.branch_name,
            git_head_object_id: git_config.git_head_object_id,
            git_repo: git_config.repository,
            git_should_force_push: cli_args.git_should_force_push,
            github_app_id: github_app_id,
            github_app_installation_id: github_app_installation_id,
            github_app_private_key: github_app_private_key_pem_data,
            github_repo_owner: cli_args.github_repo_owner,
            github_repo_name: cli_args.github_repo_name,
        })
    }
}

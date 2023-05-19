use std::{env, fmt};

use clap::Parser;
use git2::Repository;
use jsonwebtoken::EncodingKey;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::github::GitHubRepo;

/// ghommit: GitHub commit
#[derive(Debug)]
#[derive(clap::Parser)]
#[command(name = "ghommit")]
struct CommandLineArgumentsRaw {
    /// Commit message
    #[arg(long, short, required = true)]
    message: String,

    /// Force push
    #[arg(long, short, default_value = "false")]
    force: bool,
}

#[derive(Debug)]
pub struct CommandLineArguments {
    pub commit_message: String,
    pub git_should_force_push: bool,
}

impl CommandLineArguments {
    pub fn gather() -> Result<CommandLineArguments, String> {
        let raw_args = match CommandLineArgumentsRaw::try_parse() {
            Ok(res) => res,
            Err(e) => Err(e.to_string())?,
        };

        Ok(CommandLineArguments {
            commit_message: raw_args.message,
            git_should_force_push: raw_args.force,
        })
    }
}

static GITHUB_URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^git@github\.com:(.+)/(.+)$").unwrap()
});

pub struct GitConfig {
    pub branch_name: String,
    pub git_head_object_id: String,
    pub github_repo: GitHubRepo,
    pub repository: Repository,
}

impl GitConfig {
    pub fn gather(maybe_repo: Result<Repository, git2::Error>) -> Result<GitConfig, String> {
        match maybe_repo {
            Ok(repo) => {
                let (branch_name, head_object_id, github_repo) = match repo.head() {
                    Ok(head) => {
                        let branch_name = match head.shorthand() {
                            Some(name) => name.to_owned(),
                            None => Err("Git repository HEAD branch name doesn't exist or is invalid".to_owned())?,
                        };

                        let head_object_id = match head.peel_to_commit() {
                            Ok(commit) => commit.id().to_string(),
                            Err(_) => Err(format!("Could not resolve commit for branch {}", branch_name))?,
                        };

                        let github_repo = {
                            let remote = repo.find_remote("origin")
                                .map_err(|_| format!("No remote associated with branch {:?}", branch_name))?;
                            let push_url_str = remote.pushurl()
                                .or_else(|| remote.url())
                                .ok_or_else(|| format!("No push URL for remote asociated with branch {:?}", branch_name))?;

                            match GITHUB_URL_REGEX.captures(push_url_str) {
                                Some(captures) => {
                                    // - If there are captures, and there are,
                                    //   three (and only three) are guaranteed
                                    //   to exist:
                                    //   - <the whole string>
                                    //   - (.+)
                                    //   - (.+)
                                    let owner = captures[1].to_string();
                                    let name = captures[2].trim_end_matches('/').trim_end_matches(".git").to_string();

                                    GitHubRepo {
                                        owner: owner,
                                        name: name,
                                    }
                                },
                                None => Err(format!("Expected remote URL to match git@github.com/repo_owner/repo_name: {:?}", push_url_str))?,
                            }
                        };

                        (branch_name, head_object_id, github_repo)
                    },
                    Err(_) => Err("Git repository doesn't have a HEAD".to_owned())?,
                };

                Ok(GitConfig {
                    branch_name: branch_name,
                    git_head_object_id: head_object_id,
                    github_repo: github_repo,
                    repository: repo,
                })
            },
            Err(_) => Err("Not in a Git repository".to_owned()),
        }
    }
}

pub struct EnvironmentVariableConfig {
    pub github_app_id: u64,
    pub github_app_installation_id: u64,
    pub github_app_private_key: EncodingKey,
}

impl EnvironmentVariableConfig {
    fn environment_variable(name: &str) -> Result<String, String> {
        match env::var(name) {
            Ok(result) => Ok(result),
            Err(_) => Err(format!("Environment variable not set: {}", name)),
        }
    }

    fn environment_variable_rsa_private_key(name: &str) -> Result<EncodingKey, String> {
        let pem_data = Self::environment_variable(name)?;

        match EncodingKey::from_rsa_pem(pem_data.as_bytes()) {
            Ok(key) => Ok(key),
            Err(_) => Err(format!("Environment variable {} is not valid RSA private key", name)),
        }
    }

    fn environment_variable_u64(name: &str) -> Result<u64, String> {
        let as_string = Self::environment_variable(name)?;

        match as_string.parse::<u64>() {
            Ok(result) => Ok(result),
            Err(_) => Err(format!("Environment variable {} cannot be parsed as u64: {}", name, as_string)),
        }
    }

    pub fn gather() -> Result<EnvironmentVariableConfig, String> {
        Ok(EnvironmentVariableConfig {
            github_app_id: Self::environment_variable_u64("GHOMMIT_GITHUB_APP_ID")?,
            github_app_installation_id: Self::environment_variable_u64("GHOMMIT_GITHUB_APP_INSTALLATION_ID")?,
            github_app_private_key: Self::environment_variable_rsa_private_key("GHOMMIT_GITHUB_APP_PRIVATE_KEY_PEM_DATA")?,
        })
    }
}

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

impl Config {
    pub fn from(cli_args: CommandLineArguments, git_config: GitConfig, env_config: EnvironmentVariableConfig) -> Config {
        Config {
            commit_message: cli_args.commit_message,
            git_branch_name: git_config.branch_name,
            git_head_object_id: git_config.git_head_object_id,
            git_repo: git_config.repository,
            git_should_force_push: cli_args.git_should_force_push,
            github_app_id: env_config.github_app_id,
            github_app_installation_id: env_config.github_app_installation_id,
            github_app_private_key: env_config.github_app_private_key,
            github_repo_owner: git_config.github_repo.owner,
            github_repo_name: git_config.github_repo.name,
        }
    }

    /// Gathers the config from command line arguments, the Git repository, and
    /// from environment variables.
    pub fn gather(maybe_repo: Result<Repository, git2::Error>) -> Result<Config, String> {
        let cli_args = CommandLineArguments::gather()?;
        let git_config = GitConfig::gather(maybe_repo)?;
        let env_config = EnvironmentVariableConfig::gather()?;

        let config = Self::from(cli_args, git_config, env_config);
        Ok(config)
    }
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

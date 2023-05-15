#[cfg(test)]
pub mod test_utils {
    use std::fs::{self, File};
    use std::io::Write;
    use std::os::unix;
    use std::path::{PathBuf, Path};

    use git2::{Repository, Oid, Signature};
    use once_cell::sync::Lazy;
    use sha1::{Digest, Sha1};
    use tempfile::{TempDir, tempdir};

    pub static DELETED_FILE_OID: Lazy<Oid> = Lazy::new(|| {
        oid_from_str("0000000000000000000000000000000000000000")
    });

    pub fn oid_from_str(hash_string: &str) -> Oid {
        git2::Oid::from_str(&hash_string)
            .expect(&format!("Could not convert string {} to Oid", hash_string))
    }

    pub fn path_to_str(path: &Path) -> &str {
        path.to_str()
            .expect(&format!("Unable to convert path {:?} to a string", path))
    }

    /// `git hash-object --stdin` approximation
    pub fn git_hash_object_stdin(content: &str) -> Oid {
        let header = format!("blob {}\0", content.len());

        let mut hasher = Sha1::new();

        hasher.update(header.as_bytes());
        hasher.update(content.as_bytes());

        let hash = hasher.finalize();

        let hash_string = base16ct::lower::encode_string(&hash);

        oid_from_str(&hash_string)
    }

    pub struct TempGitRepo<'a> {
        pub directory: TempDir,
        pub repo: Repository,
        pub signature: Signature<'a>,
    }

    impl TempGitRepo<'_> {
        fn initialize_head(repo: &Repository, signature: &Signature) -> Oid {
            let tree_builder = repo.treebuilder(None)
                .expect(&format!("Unable to create tree builder for repo at {:?}", repo.path()));

            let tree_oid = tree_builder.write()
                .expect(&format!("Unable to write tree for repo at {:?}", repo.path()));

            let tree = repo.find_tree(tree_oid)
                .expect(&format!("Unable to find tree with ID {} for repo at {:?}", tree_oid, repo.path()));

            let parents = [];

            repo.commit(Some("HEAD"), &signature, &signature, "Initial commmit", &tree, &parents)
                .expect(&format!("Unable to commit for repo at {:?}", repo.path()))
        }

        pub fn new() -> TempGitRepo<'static> {
            let dir = tempdir().expect("Failed to create a temporary directory");
            let repo = Repository::init(&dir).expect(&format!("Failed to initialize a git repository in {:?}", dir.path()));

            let name = "ghommit";
            let email = "ghommit@example.com";

            let signature = Signature::now(&name, &email)
                .expect(&format!("Unable to create signature for {} <{}> for repo at {:?}", name, email, repo.path()));

            Self::initialize_head(&repo, &signature);

            TempGitRepo {
                directory: dir,
                repo: repo,
                signature: signature,
            }
        }

        pub fn create_or_replace_symlink_file(&self, filename: &str, path_to_link_to: &str) -> PathBuf {
            let link_file_absolute_path = self.directory.path().join(&filename);

            unix::fs::symlink(path_to_link_to, &link_file_absolute_path)
                .expect(&format!("Failed to create symlink {:?} pointing to {:?} in {:?}", link_file_absolute_path, filename, self.directory));

            let relative_path = link_file_absolute_path.strip_prefix(&self.directory)
                .expect(&format!("Failed to strip prefix {:?} from {:?}", self.directory, link_file_absolute_path));

            relative_path.to_path_buf()
        }

        pub fn create_or_replace_blob_file(&self, filename: &str, contents: &[u8]) -> PathBuf {
            let file_path = self.directory.path().join(&filename);

            // - This will be an error if it doesn't exist, which is fine to
            //   ignore
            if let Err(_) = fs::remove_file(&file_path) {}

            let mut file = File::create(&file_path)
                .expect(&format!("Failed to create file {} in {:?}", filename, self.directory));

            file.write_all(contents)
                .expect(&format!("Unable to write to file {:?}", file));

            let relative_path = file_path.strip_prefix(&self.directory)
                .expect(&format!("Failed to strip prefix {:?} from {:?}", self.directory, file_path));

            relative_path.to_path_buf()
        }

        pub fn git_add(&self, path: &Path) {
            let mut index = self.repo.index()
                .expect(&format!("Unable to access index of repo in {:?}", self.directory));

            index.add_path(&path)
                .expect(&format!("Unable to add path {:?} to index in memory", path));

            index.write()
                .expect(&format!("Unable to add path {:?} to index on disk", path));
        }

        pub fn git_commit(&self, message: &str) -> Oid {
            let mut index = self.repo.index()
                .expect(&format!("Unable to access index of repo in {:?}", self.directory));

            let head = self.repo.head()
                .expect(&format!("Unable to access head of repo in {:?}", self.directory));

            let tree_oid = index.write_tree()
                .expect(&format!("Unable to write tree of repo in {:?}", self.directory));

            let tree = self.repo.find_tree(tree_oid)
                .expect(&format!("Unable to find tree with ID {} for repo at {:?}", tree_oid, self.repo.path()));

            let head_oid = head.target()
                .expect(&format!("Unable to get OID of head for repo at {:?}", self.directory));

            let parent_commit = self.repo.find_commit(head_oid)
                .expect(&format!("Unable to get parent commit for repo at {:?}", self.directory));

            self.repo.commit(Some("HEAD"), &self.signature, &self.signature, message, &tree, &[&parent_commit])
                .expect(&format!("Unable to commit for repo at {:?}", self.repo.path()))
        }

        pub fn git_rm(&self, path: &Path) {
            let mut index = self.repo.index()
                .expect(&format!("Unable to access index of repo in {:?}", self.directory));

            index.remove_path(&path)
                .expect(&format!("Unable to remove path {:?} from index in memory", path));

            index.write()
                .expect(&format!("Unable to remove path {:?} from index on disk", path));
        }
    }
}

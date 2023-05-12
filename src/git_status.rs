use git2::{Delta, DiffOptions, FileMode, Repository, Index, ObjectType};

#[derive(Debug, Eq, PartialEq)]
pub struct PathStatus {
    pub delta: Delta,
    pub file_mode: FileMode,
    pub object_type: Option<ObjectType>,
    pub original_path: Option<String>,
    pub path: String,
}

/// Currently conflicts are not supported. Should that change in the future,
/// this function would likely be removed and other changes would need to be
/// made to accommodate.
///
/// https://git-scm.com/docs/revisions/2.39.3#Documentation/revisions.txt-emltngtltpathgtemegem0READMEememREADMEem
fn stage_number(index: &Index) -> Result<i32, String> {
    if index.has_conflicts() {
        Err(format!("Handling conflicts is not supported, and conflicts were detected"))
    } else {
        Ok(0)
    }
}

pub fn git_status(repo: &Repository) -> Result<Vec<PathStatus>, String> {
    let index = repo.index()
        .map_err(|e| format!("Unable to read git index: {}", e))?;

    let stage_number = stage_number(&index)?;

    let head = repo.head()
        .map_err(|e| format!("Unable to read git head: {}", e))?;
    let head_tree = head.peel_to_tree()
        .map_err(|e| format!("Unable to peel git head to tree: {}", e))?;

    let mut diff_options = DiffOptions::new();
    diff_options.include_typechange(true);
    diff_options.include_typechange_trees(true);

    let diff = repo.diff_tree_to_index(
        Some(&head_tree),
        Some(&index),
        Some(&mut diff_options),
    ).map_err(|e| format!("Unable to create diff between head tree and index: {}", e))?;

    let mut changes: Vec<PathStatus> = vec![];

    for diff_delta in diff.deltas() {
        let delta = diff_delta.status();
        let file_mode = diff_delta.new_file().mode();

        let new_path = diff_delta.new_file().path()
            .ok_or_else(|| format!("Delta is missing path: {:?}", diff_delta))?;

        let path_string = match new_path.to_str() {
            Some(path_str) => path_str.to_owned(),
            None => Err(format!("Path could not be converted to a string: {:?}", new_path))?,
        };

        let object_type = match index.get_path(new_path, stage_number) {
            Some(index_entry) => {
                match repo.find_object(index_entry.id, None) {
                    Ok(object) => object.kind(),
                    Err(_) => Err(format!("Unable to find object with ID {}", index_entry.id))?,
                }
            },
            // - Deleted files will not have a tree entry
            None => None,
        };

        let original_path = match diff_delta.old_file().path() {
            Some(path) => {
                match path.to_str() {
                    Some(path_str) => Some(path_str.to_owned()),
                    None => Err(format!("Path could not be converted to a string: {:?}", path))?,
                }
            }
            None => None,
        };

        let path_status = PathStatus {
            delta: delta,
            file_mode: file_mode,
            object_type: object_type,
            original_path: original_path,
            path: path_string,
        };

        changes.push(path_status);
    }

    Ok(changes)
}

#[cfg(test)]
mod git_status_tests {
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use git2::{Oid, Repository, Signature, FileMode};
    use tempfile::{TempDir, tempdir};

    use super::{PathStatus, git_status};

    struct TempGitRepo<'a> {
        directory: TempDir,
        repo: Repository,
        signature: Signature<'a>,
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

        pub fn create_or_replace_file(&self, filename: &str, contents: &[u8]) -> PathBuf {
            let file_path = self.directory.path().join(&filename);

            let mut file = File::create(&file_path)
                .expect(&format!("Failed to create file {} in {:?}", filename, self.directory));

            file.write_all(contents)
                .expect(&format!("Unable to write to file {:?}", file));

            let relative_path = file_path.strip_prefix(&self.directory)
                .expect(&format!("Failed to strip prefix {:?} from {:?}", self.directory, file));

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

    /// This is O(n^2). Since inputs are small, this shouldn't be an issue, but
    /// if it becomes an issue, consider implementing the traits necessary so
    /// that the `Vec`s can either be:
    /// - Sorted
    /// - Converted to sets
    fn assert_eq_order_independent(a: &Vec<PathStatus>, b: &Vec<PathStatus>) {
        assert_eq!(a.len(), b.len());

        assert_eq!(a.iter().all(|item| b.contains(item)), true);
        assert_eq!(b.iter().all(|item| a.contains(item)), true);
    }

    #[test]
    fn added_file() {
        let repo = TempGitRepo::new();
        let foo = repo.create_or_replace_file("foo", "foo\n".as_bytes());

        repo.git_add(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = foo.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", foo));

            vec![
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Blob,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    fn modified_file() {
        let repo = TempGitRepo::new();
        let foo = repo.create_or_replace_file("foo", "foo\n".as_bytes());

        repo.git_add(&foo);
        repo.git_commit("Adding foo");

        let foo = repo.create_or_replace_file("foo", "foo\nfoo\n".as_bytes());

        repo.git_add(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = foo.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", foo));

            vec![
                PathStatus {
                    delta: git2::Delta::Modified,
                    file_mode: FileMode::Blob,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    fn deleted_file() {
        let repo = TempGitRepo::new();
        let foo = repo.create_or_replace_file("foo", "foo\n".as_bytes());

        repo.git_add(&foo);
        repo.git_commit("Adding foo");

        repo.git_rm(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = foo.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", foo));

            vec![
                PathStatus {
                    delta: git2::Delta::Deleted,
                    file_mode: FileMode::Unreadable,
                    object_type: None,
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    fn multiple_changes() {
        let repo = TempGitRepo::new();
        let foo = repo.create_or_replace_file("foo", "foo\n".as_bytes());
        let bar = repo.create_or_replace_file("bar", "bar\n".as_bytes());

        repo.git_add(&foo);
        repo.git_add(&bar);
        repo.git_commit("Adding foo and bar");

        let bar = repo.create_or_replace_file("bar", "bar\nbar\n".as_bytes());
        let baz = repo.create_or_replace_file("baz", "baz\n".as_bytes());

        repo.git_add(&bar);
        repo.git_add(&baz);
        repo.git_rm(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let foo_path = foo.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", foo));
            let bar_path = bar.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", bar));
            let baz_path = baz.to_str()
                .expect(&format!("Unable to convert path {:?} to a string", baz));

            vec![
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Blob,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(baz_path.to_owned()),
                    path: baz_path.to_owned(),
                },
                PathStatus {
                    delta: git2::Delta::Modified,
                    file_mode: FileMode::Blob,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(bar_path.to_owned()),
                    path: bar_path.to_owned(),
                },
                PathStatus {
                    delta: git2::Delta::Deleted,
                    file_mode: FileMode::Unreadable,
                    path: foo_path.to_owned(),
                    object_type: None,
                    original_path: Some(foo_path.to_owned()),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }
}

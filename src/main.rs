use core::panic;
use git2::{DiffLineType, DiffOptions, Object, ObjectType, Patch, Repository};
use glob::glob;
use notify_debouncer_full::{new_debouncer, notify::*, DebounceEventResult};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{collections::HashMap, collections::HashSet, fs};
use tree_sitter::{InputEdit, Point, Query, QueryCapture, QueryCursor, Tree};

struct BetterDiff {
    path: String,
    start_offset: usize,
    deletion_end: usize,
    addition_end: usize,
    start_point: Point,
    addition_point: Point,
    deletion_point: Point,
}

impl std::fmt::Display for BetterDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BetterDiff: {} {} {} {}",
            self.path, self.start_offset, self.deletion_end, self.addition_end,
        )
    }
}

fn content_from_hunk(patch: &Patch, hunk_i: usize) -> (String, String, usize, usize) {
    let mut addition = String::new();
    let mut deletion = String::new();
    let num_lines = patch.num_lines_in_hunk(hunk_i).unwrap();
    let mut latest_addition: Option<String> = None;
    let mut latest_deletion: Option<String> = None;
    for line_i in 0..num_lines {
        let line = patch.line_in_hunk(hunk_i, line_i).unwrap();
        let string_to_push = std::str::from_utf8(line.content()).unwrap();
        match line.origin_value() {
            DiffLineType::Addition => {
                addition.push_str(string_to_push);
                latest_addition = Some(string_to_push.to_string());
            }
            DiffLineType::Deletion => {
                deletion.push_str(string_to_push);
                latest_deletion = Some(string_to_push.to_string());
            }
            _ => (),
        }
    }
    let last_addition_len = match latest_addition {
        Some(s) => s.len(),
        None => 0,
    };
    let last_deletion_len = match latest_deletion {
        Some(s) => s.len(),
        None => 0,
    };
    (addition, deletion, last_addition_len, last_deletion_len)
}

fn get_diff(repo: &Repository, commit: &Object) -> Vec<BetterDiff> {
    let diffs = repo
        .diff_tree_to_workdir(
            Some(&commit.as_commit().unwrap().tree().unwrap()),
            Some(&mut DiffOptions::new().context_lines(0)),
        )
        .unwrap();
    let mut v = Vec::new();
    for idx in 0..diffs.deltas().collect::<Vec<_>>().len() {
        let patch = Patch::from_diff(&diffs, idx).unwrap().unwrap();
        let path = patch.delta().old_file().path().unwrap();
        let ext = path.extension();
        match ext {
            Some(extension) => {
                if extension.to_str().unwrap() != "py" {
                    continue;
                }
            }
            None => continue,
        }
        for hunk_i in 0..patch.num_hunks() {
            patch.hunk(hunk_i).unwrap().0.new_start();
            // let num_lines = patch.num_lines_in_hunk(hunk_i).unwrap();
            let (addition, deletion, addition_end_column, deletion_end_column) =
                content_from_hunk(&patch, hunk_i);
            let start_offset = patch.line_in_hunk(hunk_i, 0).unwrap().content_offset();
            let addition_end = addition.len() + start_offset as usize;
            let deletion_end = deletion.len() + start_offset as usize;
            let start_point = (patch.hunk(hunk_i).unwrap().0.old_start(), 0);
            let addition_point = (
                patch.hunk(hunk_i).unwrap().0.new_lines() + start_point.0,
                addition_end_column,
            );
            let deletion_point = (
                patch.hunk(hunk_i).unwrap().0.old_lines() + start_point.0,
                deletion_end_column,
            );
            v.push(BetterDiff {
                path: patch
                    .delta()
                    .old_file()
                    .path()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
                start_offset: start_offset as usize,
                addition_end: addition_end,
                deletion_end: deletion_end,
                start_point: Point {
                    row: start_point.0 as usize,
                    column: start_point.1,
                },
                addition_point: Point {
                    row: addition_point.0 as usize,
                    column: addition_point.1,
                },
                deletion_point: Point {
                    row: deletion_point.0 as usize,
                    column: deletion_point.1,
                },
            });
            let num_lines = patch.num_lines_in_hunk(hunk_i).unwrap();
        }
    }
    v
}

pub fn print_tree(
    content_map: HashMap<String, String>,
    tree_map: HashMap<String, Tree>,
) -> Vec<String> {
    let mut ret: Vec<String> = Vec::new();
    tree_map.iter().for_each(|(path, tree)| {
        let mut cursor = tree.walk();
        'outer: loop {
            if cursor.node().is_named() {
                match cursor.node().kind() {
                    "function_definition" => {
                        if cursor
                            .node()
                            .child_by_field_name("name")
                            .unwrap()
                            .utf8_text(content_map[path].as_bytes())
                            .unwrap()
                            .starts_with("test")
                        {
                            println!(
                                "{:?} {:?} {:?}",
                                cursor.node(),
                                cursor
                                    .node()
                                    .utf8_text(content_map[path].as_bytes())
                                    .unwrap(),
                                cursor
                                    .node()
                                    .named_children(&mut tree.walk())
                                    .collect::<Vec<_>>() // cursor
                                                         //     .node()
                                                         //     .child_by_field_name("name")
                                                         //     .unwrap()
                                                         //     .utf8_text(file_content_map[path].as_bytes())
                            );
                            ret.push(
                                cursor
                                    .node()
                                    .child_by_field_name("name")
                                    .unwrap()
                                    .utf8_text(content_map[path].as_bytes())
                                    .unwrap()
                                    .to_string(),
                            )
                        }
                    }
                    _ => (),
                }
            }

            if cursor.goto_first_child() || cursor.goto_next_sibling() {
                continue;
            }

            loop {
                if !cursor.goto_parent() {
                    break 'outer;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    });
    ret
}

fn get_tests(
    content_map: HashMap<String, String>,
    tree_map: &HashMap<String, Tree>,
) -> HashSet<String> {
    let mut v: HashSet<String> = HashSet::new();
    for (path, tree) in tree_map {
        let mut q = Query::new(
            tree_sitter_python::language(),
            "(function_definition (identifier)@b ) @a",
        )
        .unwrap();
        let mut qc = QueryCursor::new();
        let qm = qc.matches(&mut q, tree.root_node(), content_map[path].as_bytes());
        qm.for_each(|query_match| {
            query_match
                .captures
                .iter()
                .for_each(|capture: &QueryCapture| {
                    let function_name = capture
                        .node
                        .utf8_text(content_map[path].as_bytes())
                        .unwrap();
                    if function_name.starts_with("test") {
                        v.insert(format!("{}::{}", path, function_name.to_string()));
                    }
                })
        });
    }
    v
}

fn create_old_content_map(repo: &Repository, commit: &Object) -> HashMap<String, String> {
    let mut old_content_map: HashMap<String, String> = HashMap::new();

    commit
        .as_commit()
        .unwrap()
        .tree()
        .unwrap()
        .walk(git2::TreeWalkMode::PreOrder, |s, entry| {
            let o = entry.to_object(repo).unwrap();
            if entry.kind().unwrap() == ObjectType::Blob && entry.name().unwrap().ends_with("py") {
                let content = String::from_utf8(o.as_blob().unwrap().content().to_vec());
                let path = match s.is_empty() {
                    true => entry.name().unwrap().to_string(),
                    false => format!("{}/{}", s, entry.name().unwrap()),
                };
                old_content_map.insert(path, content.unwrap());
            }
            return 0;
        })
        .unwrap();
    old_content_map
}

fn create_new_content_map() -> HashMap<String, String> {
    let mut new_content_map = HashMap::new();
    let globbed = match glob("**/*.py") {
        Err(_) => {
            panic!("A");
        }
        Ok(paths) => paths,
    };

    for entry in globbed {
        let path = match entry {
            Err(_) => panic!("B"),
            Ok(pathbuf) => String::from(pathbuf.to_str().unwrap()),
        };
        let content = fs::read_to_string(path.clone()).unwrap();
        new_content_map.insert(path, content);
    }
    new_content_map
}

fn create_parser() -> tree_sitter::Parser {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(tree_sitter_python::language())
        .expect("Error loading Python grammar");
    parser
}

fn edit_tree(vd: Vec<BetterDiff>, tree_map: &mut HashMap<String, Tree>) {
    for d in vd {
        let t = tree_map.get_mut(&d.path).unwrap();
        t.edit(&InputEdit {
            start_byte: d.start_offset,
            old_end_byte: d.deletion_end,
            new_end_byte: d.addition_end,
            start_position: d.start_point,
            old_end_position: d.deletion_point,
            new_end_position: d.addition_point,
        });
    }
}

fn main() {
    let (tx, rx) = std::sync::mpsc::channel();

    // no specific tickrate, max debounce time 2 seconds
    let mut debouncer = new_debouncer(Duration::from_secs(2), None, tx).unwrap();

    debouncer
        .watcher()
        .watch(Path::new("."), RecursiveMode::Recursive)
        .unwrap();

    debouncer
        .cache()
        .add_root(Path::new("."), RecursiveMode::Recursive);

    // print all events and errors
    for result in rx {
        match result {
            Ok(events) => {
                if events.iter().any(|event| {
                    event
                        .paths
                        .iter()
                        .any(|path| path.extension().unwrap_or(OsStr::new("")) == "py")
                }) {
                    on_fs_event();
                };
            }
            Err(errors) => errors.iter().for_each(|error| println!("{error:?}")),
        }
    }

    return;
}

fn on_fs_event() {
    let mut tree_map: HashMap<String, Tree> = HashMap::new();

    let repo: Repository = match Repository::open(".") {
        Ok(repo) => repo,
        Err(e) => panic!("failed to open: {}", e),
    };
    let commit = repo.revparse_single("HEAD").unwrap();

    let old_content_map = create_old_content_map(&repo, &commit);
    let new_content_map = create_new_content_map();

    let mut parser = create_parser();

    for (path, content) in &old_content_map {
        let tree = parser.parse(content, None).unwrap();
        tree_map.insert(path.to_string(), tree);
    }

    let old_tests = get_tests(old_content_map.clone(), &tree_map);

    let vd = get_diff(&repo, &commit);

    edit_tree(vd, &mut tree_map);

    for (path, content) in &new_content_map {
        let tree = parser.parse(content, None).unwrap();
        tree_map.insert(path.to_string(), tree);
    }

    let new_tests = get_tests(new_content_map, &tree_map);

    let hs_diff = new_tests.difference(&old_tests);

    let mut tests_to_run = String::new();
    hs_diff.for_each(|diff| tests_to_run.push_str(diff.as_str()));

    println!("Running {}", tests_to_run);

    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("coverage run -m pytest {}", tests_to_run))
        .output()
        .expect("failed to execute process");
    let hello = output.stdout;
    println!("{}", String::from_utf8(hello).unwrap());
}

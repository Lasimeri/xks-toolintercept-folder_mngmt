//! Layout gaps, and the manifest write that closes one.
//!
//! A gap is a planned path that lies outside every folder of the manifest's
//! layout: a new top-level folder, a path under a top-level folder the layout
//! does not map, or a file in the project root whose type (by the standard's
//! extension table, `roles::plausible`) is not one a root holds. Gaps are
//! read from the manifest, that table, the standard's folder name table
//! (`roles::by_folder_name`, a hint) and the layout changes the model's
//! confident verdicts name (`Verdict::layout_change`); nothing here asks the
//! model or decides where a path goes. `extend_layout` (in `tools`) adds
//! folders to the layout under a role the caller names, through
//! `update_manifest`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::Path;

use serde_json::{Value, json};
use xtf_standard::decide::LOG_FILE;
use xtf_standard::roles::{self, Role};
use xtf_standard::{LayoutChange, MANIFEST_FILE, Manifest, Op, Place, Projection, Verdict, naming};

/// One gap: where it is, and the planned paths that fall into it.
struct Gap {
    /// `scripts/` for a folder, `bench.sh` for a root file.
    at: String,
    folder: bool,
    paths: Vec<String>,
    /// Roles the planned files' names or extensions make plausible.
    plausible: BTreeSet<Role>,
    /// The layout changes the model's confident verdicts on these paths
    /// name (`Verdict::layout_change`), distinct, with the confidence and
    /// the path of the first verdict naming each, and its suggested path.
    model: Vec<(LayoutChange, f64, String, Option<String>)>,
}

impl Gap {
    fn new(at: String, folder: bool) -> Gap {
        Gap {
            at,
            folder,
            paths: Vec::new(),
            plausible: BTreeSet::new(),
            model: Vec::new(),
        }
    }

    fn heard(&mut self, path: &str, v: Option<&Verdict>) {
        let Some(v) = v else { return };
        let Some(c) = &v.layout_change else { return };
        if self.model.iter().all(|(m, ..)| m != c) {
            self.model.push((
                c.clone(),
                v.confidence.unwrap_or(0.0),
                path.to_string(),
                v.suggested_path.clone(),
            ));
        }
    }
}

/// The path's components, `.` and empty ones dropped.
fn parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect()
}

/// The roles a file name makes plausible: the name convention first
/// (`test_*`, `bench_*`), then the extension table. Empty when neither says
/// anything.
fn plausible_of(name: &str) -> Vec<Role> {
    if let Some(r) = roles::by_name(name) {
        return vec![r];
    }
    roles::plausible(name)
        .map(<[Role]>::to_vec)
        .unwrap_or_default()
}

/// The gaps among `planned` (each path with its verdict, when there is
/// one), one entry per top-level folder or root file, in path order.
/// Deletions never make a gap.
pub fn gaps<'a>(
    m: &Manifest,
    planned: impl IntoIterator<Item = (&'a Projection, Option<&'a Verdict>)>,
) -> Vec<Value> {
    let mut found: BTreeMap<String, Gap> = BTreeMap::new();
    for (p, v) in planned {
        if p.op == Op::Delete {
            continue;
        }
        let ps = parts(&p.path);
        let Some(&top) = ps.first() else {
            continue;
        };
        let dir = p.op == Op::CreateDir;
        if ps.len() == 1 && !dir {
            // A root file: a gap only when its type is known and is not one
            // the root holds (config), e.g. a script or a source file.
            if naming::is_conventional(top) {
                continue;
            }
            let pl = plausible_of(top);
            if pl.is_empty() || pl.contains(&Role::Config) {
                continue;
            }
            let g = found
                .entry(top.to_string())
                .or_insert_with(|| Gap::new(top.to_string(), false));
            g.paths.push(top.to_string());
            g.plausible.extend(pl);
            g.heard(top, v);
            continue;
        }
        // In (or being) a top-level folder: a gap when the layout and the
        // ignore list leave that folder unmapped.
        let probe = if ps.len() == 1 {
            format!("{top}/-")
        } else {
            ps.join("/")
        };
        if !matches!(m.place(Path::new(&probe)), Place::Unmapped(_)) {
            continue;
        }
        let at = format!("{top}/");
        let g = found
            .entry(at.clone())
            .or_insert_with(|| Gap::new(at, true));
        let shown = if dir {
            format!("{}/", ps.join("/"))
        } else {
            ps.join("/")
        };
        g.heard(&shown, v);
        g.paths.push(shown);
        if !dir {
            g.plausible.extend(plausible_of(ps[ps.len() - 1]));
        }
    }
    let have = m.roles();
    found.into_values().map(|g| describe(m, &have, g)).collect()
}

/// `role (dir/)` for each role, its usual folder named.
fn usual(rs: &[Role]) -> String {
    rs.iter()
        .map(|r| format!("{} ({}/)", r.key(), r.default_dir()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe(m: &Manifest, have: &[Role], g: Gap) -> Value {
    let real = |r: &&Role| !matches!(r, Role::Out | Role::Config);
    let lacks: Vec<Role> = g
        .plausible
        .iter()
        .filter(real)
        .filter(|r| !have.contains(*r))
        .copied()
        .collect();
    let has: Vec<Role> = g
        .plausible
        .iter()
        .filter(real)
        .filter(|r| have.contains(*r))
        .copied()
        .collect();
    let has_dirs: Vec<String> = has.iter().map(|r| format!("{}/", m.dir_for(*r))).collect();
    let top = g.at.trim_end_matches('/');
    // The folder name table of the standard (the one `xtf init` uses): a
    // hint, never a decision.
    let usual_role = if g.folder {
        roles::by_folder_name(top).filter(|r| *r != Role::Config)
    } else {
        None
    };
    let next = if !g.model.is_empty() {
        // The model named the role: its verdict is the step.
        g.model
            .iter()
            .map(|(c, conf, path, moved)| {
                let mut s = format!(
                    "the model reads {path} as {} (confidence {conf:.2}): extend_layout \
                     {{role: \"{}\", dirs: [\"{}\"]}} records {}/ in the layout",
                    c.role, c.role, c.dir, c.dir
                );
                if let Some(to) = moved {
                    s.push_str(&format!(", then write it as {to}"));
                }
                s
            })
            .collect::<Vec<_>>()
            .join("; ")
    } else if g.folder {
        let mut s = format!(
            "the layout does not map {}: record the role it plays with extend_layout \
             {{role, dirs: [\"{top}\"]}}",
            g.at
        );
        if let Some(r) = usual_role {
            s.push_str(&format!(
                "; by its name, {} usually plays {}",
                g.at,
                r.key()
            ));
        }
        if !lacks.is_empty() {
            s.push_str(&format!(
                "; roles its planned files' types play that the layout lacks: {}",
                lacks.iter().map(|r| r.key()).collect::<Vec<_>>().join(", ")
            ));
        }
        if !has_dirs.is_empty() {
            s.push_str(&format!(
                "; or plan them in {} instead",
                has_dirs.join(" or ")
            ));
        }
        s
    } else {
        let mut s = format!("{} is not a file the project root holds", g.at);
        if !lacks.is_empty() {
            s.push_str(&format!(
                "; roles it plays that the layout lacks: {}: add one with extend_layout \
                 {{role, dirs}}, then plan it in that folder",
                usual(&lacks)
            ));
        }
        if !has_dirs.is_empty() {
            s.push_str(&format!(
                "; {} plan it in {}",
                if lacks.is_empty() { "" } else { "or" },
                has_dirs.join(" or ")
            ));
        }
        s.replace(";  plan", "; plan")
    };
    json!({
        "at": g.at,
        "kind": if g.folder { "unmapped_folder" } else { "root_file" },
        "paths": g.paths,
        "layout_change": g.model.iter().map(|(c, conf, path, moved)| json!({
            "role": c.role, "dir": c.dir, "confidence": conf, "from": path, "suggested_path": moved,
        })).collect::<Vec<_>>(),
        "usual_role": usual_role.map(Role::key),
        "layout_lacks": lacks.iter().map(|r| r.key()).collect::<Vec<_>>(),
        "layout_has": has.iter().map(|r| json!({"role": r.key(), "dir": format!("{}/", m.dir_for(*r))})).collect::<Vec<_>>(),
        "next": next,
    })
}

/// `; layout gaps: a/, b.sh (+N more)` for a summary line, empty when none.
pub fn line_note(gaps: &[Value]) -> String {
    if gaps.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = gaps
        .iter()
        .filter_map(|g| g.get("at").and_then(Value::as_str))
        .collect();
    let shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    let more = names.len().saturating_sub(3);
    format!(
        "; layout gap{} {shown}{} (see layout_gaps)",
        if names.len() == 1 { "" } else { "s" },
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    )
}

/// A folder of an `extend_layout` call, relative to the project root, `/`
/// separated, without `.` parts or a trailing `/`.
pub fn plain_dir(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.starts_with('/') || t.starts_with('~') {
        return Err(format!(
            "`{t}`: layout folders are relative to the project root"
        ));
    }
    let ps = parts(t);
    if ps.is_empty() {
        return Err("a layout folder needs a name".into());
    }
    if ps.contains(&"..") {
        return Err(format!("`{t}`: a layout folder cannot leave the project"));
    }
    Ok(ps.join("/"))
}

mod sys {
    unsafe extern "C" {
        /// flock(2): `fd` is only read as a number; a bad one is EBADF.
        pub safe fn flock(fd: i32, operation: i32) -> i32;
    }
}

/// flock(2) LOCK_EX.
const LOCK_EX: i32 = 2;

/// Load the manifest of `root`, let `change` edit it, and write it back,
/// holding an exclusive lock on `.xks/layout.lock` throughout, so two
/// sessions extending one layout never lose each other's folders. The new
/// text goes to a temporary file that replaces the manifest in one rename:
/// a reader (the hook of another session) sees the old manifest or the new
/// one, never a part. The text is what `Manifest::save` writes; when it
/// equals the file's, nothing is written. `change` returns its result, or
/// Err to leave the manifest as it was.
pub fn update_manifest<T>(
    root: &Path,
    change: impl FnOnce(&mut Manifest) -> Result<T, String>,
) -> Result<T, String> {
    let lock_path = root.join(LOG_FILE).with_file_name("layout.lock");
    if let Some(d) = lock_path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_path)
        .map_err(|e| format!("{}: {e}", lock_path.display()))?;
    loop {
        if sys::flock(lock.as_raw_fd(), LOCK_EX) == 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("{}: {e}", lock_path.display()));
        }
    }
    let mut m = Manifest::load(root)?;
    let out = change(&mut m)?;
    let text = serde_json::to_string_pretty(&m).expect("manifest to JSON") + "\n";
    let path = root.join(MANIFEST_FILE);
    if std::fs::read(&path).is_ok_and(|now| now == text.as_bytes()) {
        return Ok(out);
    }
    let tmp = root.join(format!(".{MANIFEST_FILE}.{}.tmp", std::process::id()));
    // Only the lock holder writes this name (it carries the process id, and
    // the lock is held), so truncating a leftover of a crashed run is safe.
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        // The temporary file is this call's own; the manifest is untouched.
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    drop(lock);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let mut m = Manifest::new("p", "A command-line tool.", vec!["rust".into()]);
        m.layout = [
            (Role::Source, vec!["src".to_string()]),
            (Role::Tests, vec!["tests".to_string()]),
            (Role::Docs, vec!["docs".to_string()]),
            (Role::Out, vec!["out".to_string()]),
        ]
        .into_iter()
        .collect();
        m
    }

    fn proj(op: Op, path: &str) -> Projection {
        Projection {
            op,
            path: path.into(),
            from: None,
            summary: String::new(),
            tool: None,
        }
    }

    #[test]
    fn gaps_are_paths_outside_the_layout() {
        let m = manifest();
        let planned = [
            proj(Op::CreateFile, "Cargo.toml"),
            proj(Op::CreateFile, "README.md"),
            proj(Op::CreateFile, "src/main.rs"),
            proj(Op::CreateFile, "docs/usage.md"),
            proj(Op::CreateDir, "tests/fixtures"),
            proj(Op::CreateFile, "out/report.md"),
            proj(Op::CreateFile, "rustfmt.toml"),
            proj(Op::CreateFile, "bench.sh"),
            proj(Op::CreateDir, "scripts"),
            proj(Op::CreateFile, "scripts/release.sh"),
            proj(Op::CreateFile, "notes"),
            proj(Op::Delete, "old/x.rs"),
            proj(Op::CreateFile, "target/debug/x"),
        ];
        let g = gaps(&m, planned.iter().map(|p| (p, None)));
        let at: Vec<&str> = g.iter().map(|v| v["at"].as_str().unwrap()).collect();
        // Not gaps: conventional and config root files, layout folders, out/,
        // a name without a known type, a deletion, and target/ (in the
        // manifest's default ignore list).
        assert_eq!(at, ["bench.sh", "scripts/"], "{g:#?}");
        assert_eq!(g[0]["kind"], "root_file");
        assert_eq!(
            g[0]["layout_lacks"],
            json!(["examples", "tools", "integrations"])
        );
        assert_eq!(
            g[0]["layout_has"],
            json!([{"role": "tests", "dir": "tests/"}])
        );
        let next = g[0]["next"].as_str().unwrap();
        assert!(
            next.contains("tools (tools/)") && next.contains("or plan it in tests/"),
            "{next}"
        );
        assert_eq!(g[1]["kind"], "unmapped_folder");
        assert_eq!(g[1]["paths"], json!(["scripts/", "scripts/release.sh"]));
        assert_eq!(
            g[1]["layout_lacks"],
            json!(["examples", "tools", "integrations"])
        );
        let next = g[1]["next"].as_str().unwrap();
        assert!(next.contains("dirs: [\"scripts\"]"), "{next}");
        // The standard's folder name table gives a hint for scripts/.
        assert_eq!(g[1]["usual_role"], "tools");
        assert!(
            next.contains("by its name, scripts/ usually plays tools"),
            "{next}"
        );
        assert_eq!(g[0]["usual_role"], Value::Null);
        assert_eq!(g[0]["layout_change"], json!([]));
        assert_eq!(
            line_note(&g),
            "; layout gaps bench.sh, scripts/ (see layout_gaps)"
        );
        assert_eq!(line_note(&[]), "");
    }

    #[test]
    fn the_models_layout_change_is_the_next_step() {
        let m = manifest();
        let v = |role: &str, dir: &str, moved: Option<&str>| Verdict {
            action: xtf_standard::Action::Suggest,
            notes: Vec::new(),
            suggested_path: moved.map(str::to_string),
            role: Some(role.into()),
            confidence: Some(0.8893),
            layout_change: Some(LayoutChange {
                role: role.into(),
                dir: dir.into(),
            }),
        };
        let folder = proj(Op::CreateDir, "scripts");
        let file = proj(Op::CreateFile, "scripts/release.sh");
        let root = proj(Op::CreateFile, "bench.sh");
        let (vf, vr) = (
            v("tools", "scripts", None),
            v("tools", "tools", Some("tools/bench.sh")),
        );
        // Two verdicts naming the same change count once.
        let g = gaps(
            &m,
            [(&root, Some(&vr)), (&folder, Some(&vf)), (&file, Some(&vf))],
        );
        assert_eq!(
            g[1]["layout_change"],
            json!([{"role": "tools", "dir": "scripts", "confidence": 0.8893,
                    "from": "scripts/", "suggested_path": null}])
        );
        let next = g[1]["next"].as_str().unwrap();
        assert_eq!(
            next,
            "the model reads scripts/ as tools (confidence 0.89): extend_layout {role: \"tools\", \
             dirs: [\"scripts\"]} records scripts/ in the layout"
        );
        let next = g[0]["next"].as_str().unwrap();
        assert!(
            next.ends_with("records tools/ in the layout, then write it as tools/bench.sh"),
            "{next}"
        );
    }

    #[test]
    fn a_name_convention_counts_before_the_extension() {
        let m = manifest();
        let g = gaps(&m, [(&proj(Op::CreateFile, "bench_convert.rs"), None)]);
        assert_eq!(g[0]["layout_lacks"], json!(["benches"]));
        assert_eq!(g[0]["layout_has"], json!([]));
        let next = g[0]["next"].as_str().unwrap();
        assert!(
            !next.contains("; plan") && !next.contains("or plan"),
            "{next}"
        );
    }

    #[test]
    fn layout_folders_are_plain_relative_paths() {
        assert_eq!(plain_dir(" ./scripts/ ").unwrap(), "scripts");
        assert_eq!(plain_dir("src//tui/").unwrap(), "src/tui");
        assert!(plain_dir("/abs").is_err());
        assert!(plain_dir("~/x").is_err());
        assert!(plain_dir("a/../../b").is_err());
        assert!(plain_dir(" / ").is_err());
        assert!(plain_dir("").is_err());
    }

    #[test]
    fn manifest_updates_are_whole_and_serialized() {
        let root = std::env::temp_dir().join(format!("xtf-layout-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        manifest().save(&root).unwrap();
        // 8 threads add one folder each at once: none is lost.
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    update_manifest(&root, |m| {
                        m.layout
                            .entry(Role::Tools)
                            .or_default()
                            .push(format!("t{i}"));
                        Ok(())
                    })
                    .unwrap()
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let m = Manifest::load(&root).unwrap();
        let mut t = m.layout[&Role::Tools].clone();
        t.sort();
        assert_eq!(t, (0..8).map(|i| format!("t{i}")).collect::<Vec<_>>());
        // An Err from the change writes nothing.
        let before = std::fs::read(root.join(MANIFEST_FILE)).unwrap();
        let r: Result<(), String> = update_manifest(&root, |m| {
            m.layout.clear();
            Err("refused".into())
        });
        assert_eq!(r.unwrap_err(), "refused");
        assert_eq!(std::fs::read(root.join(MANIFEST_FILE)).unwrap(), before);
        // The text is Manifest::save's.
        let mut saved = Manifest::load(&root).unwrap();
        saved.name = "q".into();
        update_manifest(&root, |m| {
            m.name = "q".into();
            Ok(())
        })
        .unwrap();
        let ours = std::fs::read(root.join(MANIFEST_FILE)).unwrap();
        saved.save(&root).unwrap();
        assert_eq!(ours, std::fs::read(root.join(MANIFEST_FILE)).unwrap());
        std::fs::remove_dir_all(&root).ok();
    }
}

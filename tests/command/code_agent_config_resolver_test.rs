//! W4-06: Code/Agent config resolver scope/precedence contract.
//!
//! L1 — deterministic. Does not lift linked-worktree preflight (W4-08).

use std::{fs, path::Path};

use libra::{
    internal::{
        ai::sources::resolver::{
            ConfigLayer, ConfigResolveError, resolve_config_dir, resolve_config_file,
            surface_by_location,
        },
        config_ownership::{ConfigConsumerKind, ConfigOwner},
        worktree_scope::{RequestScope, WorktreeScope},
    },
    utils::test::ChangeDirGuard,
};

use super::{assert_cli_success, run_libra_command};

fn request_for(workdir: &Path) -> RequestScope {
    RequestScope::resolve(workdir.to_path_buf()).expect("RequestScope for workdir")
}

fn repo_with_linked_worktree() -> (tempfile::TempDir, tempfile::TempDir) {
    let repo = tempfile::tempdir().expect("repo");
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["init", "--vault=false"], p), "init");
    assert_cli_success(&run_libra_command(&["config", "user.name", "t"], p), "name");
    assert_cli_success(
        &run_libra_command(&["config", "user.email", "t@t"], p),
        "email",
    );
    fs::write(p.join("a.txt"), "a\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit",
    );
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], p),
        "worktree add",
    );
    (repo, parent)
}

#[test]
fn code_agent_config_resolver_scope_precedence() {
    let (repo, parent) = repo_with_linked_worktree();
    let main = repo.path();
    let wt = parent.path().join("wt");
    let main_request = request_for(main);
    let linked_request = request_for(&wt);

    let repo_sandbox = main.join(".libra").join("sandbox.toml");
    fs::write(&repo_sandbox, "[sandbox.network]\nmode = \"deny\"\n")
        .expect("write repository sandbox.toml");

    assert_eq!(main_request.scope, WorktreeScope::Main);
    let main_resolved = resolve_config_file(&main_request, "sandbox.toml").expect("main sandbox");
    assert_eq!(
        main_resolved.provenance.winning_layer,
        ConfigLayer::Repository
    );
    assert_eq!(
        main_resolved.provenance.consumer,
        ConfigConsumerKind::Security
    );
    assert!(main_resolved.overlay_bytes.is_none());
    assert!(
        String::from_utf8_lossy(&main_resolved.bytes).contains("deny"),
        "main resolves repository bytes"
    );

    assert!(linked_request.scope.is_linked());
    let linked_gitdir = wt.join(".libra");
    let overlay_sandbox = linked_gitdir.join("sandbox.toml");
    fs::write(&overlay_sandbox, "[sandbox.network]\nmode = \"full\"\n")
        .expect("write overlay sandbox that would loosen policy");

    let linked_resolved =
        resolve_config_file(&linked_request, "sandbox.toml").expect("linked sandbox");
    assert_eq!(
        linked_resolved.provenance.winning_layer,
        ConfigLayer::RepositoryWithTighteningOverlay
    );
    assert_eq!(
        linked_resolved.provenance.overlay_path.as_deref(),
        Some(overlay_sandbox.as_path())
    );
    assert!(
        String::from_utf8_lossy(&linked_resolved.bytes).contains("deny"),
        "effective bytes stay repository (never overlay-only replace)"
    );
    assert!(
        linked_resolved
            .overlay_bytes
            .as_ref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains("full")),
        "security overlay bytes must be exposed for W4-11 tighten-only merge"
    );

    // Absent security repository layer is an empty default base (sandbox loader
    // maps NotFound → SandboxConfigFile::default); overlay still exposed for
    // tighten-only composition in W4-11.
    fs::remove_file(&repo_sandbox).expect("remove repository sandbox");
    let absent = resolve_config_file(&linked_request, "sandbox.toml").expect("absent ok");
    assert!(
        absent.bytes.is_empty() && absent.repository_bytes.is_empty(),
        "absence yields empty default base"
    );
    assert_eq!(
        absent.provenance.winning_layer,
        ConfigLayer::RepositoryWithTighteningOverlay
    );
    assert!(
        absent
            .overlay_bytes
            .as_ref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains("full")),
        "overlay bytes remain visible when repository file is absent"
    );
    fs::write(&repo_sandbox, "[sandbox.network]\nmode = \"deny\"\n")
        .expect("restore repository sandbox after absence check");

    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        fs::set_permissions(&repo_sandbox, fs::Permissions::from_mode(0o000))
            .expect("chmod 000 sandbox.toml");
        let unreadable = resolve_config_file(&linked_request, "sandbox.toml");
        match unreadable {
            Err(ConfigResolveError::SecurityRepositoryUnreadable {
                location,
                repository_path,
                message,
            }) => {
                assert_eq!(location, "sandbox.toml");
                assert_eq!(repository_path, repo_sandbox);
                assert!(!message.is_empty());
                let display = ConfigResolveError::SecurityRepositoryUnreadable {
                    location,
                    repository_path: repository_path.clone(),
                    message: message.clone(),
                }
                .to_string();
                assert!(
                    display.contains("unreadable") && display.contains("sandbox.toml"),
                    "got {display}"
                );
            }
            other => panic!("expected SecurityRepositoryUnreadable, got {other:?}"),
        }
        fs::set_permissions(&repo_sandbox, fs::Permissions::from_mode(0o644))
            .expect("restore sandbox perms");

        // Dangling symlink must not look like an absent empty baseline.
        let dangling = parent.path().join("missing-sandbox-target.toml");
        let _ = fs::remove_file(&repo_sandbox);
        symlink(&dangling, &repo_sandbox).expect("dangling sandbox symlink");
        match resolve_config_file(&linked_request, "sandbox.toml") {
            Err(err) => {
                assert!(err.is_fail_closed_security(), "got {err}");
                assert!(
                    err.to_string().contains("unreadable")
                        && err.to_string().contains("sandbox.toml"),
                    "got {err}"
                );
            }
            Ok(ok) => panic!(
                "expected dangling symlink fail-closed, got {:?}",
                ok.provenance
            ),
        }
        fs::remove_file(&repo_sandbox).expect("remove dangling sandbox");
        fs::write(&repo_sandbox, "[sandbox.network]\nmode = \"deny\"\n")
            .expect("restore repository sandbox");
    }

    // Extension surfaces: overlay wins when present.
    let repo_agents = main.join(".libra").join("agents.toml");
    fs::write(&repo_agents, "name = \"repo\"\n").expect("repo agents");
    let overlay_agents = linked_gitdir.join("agents.toml");
    fs::write(&overlay_agents, "name = \"overlay\"\n").expect("overlay agents");
    let agents = resolve_config_file(&linked_request, "agents.toml").expect("agents");
    assert_eq!(agents.provenance.winning_layer, ConfigLayer::Overlay);
    assert_eq!(agents.provenance.consumer, ConfigConsumerKind::Extension);
    assert!(String::from_utf8_lossy(&agents.bytes).contains("overlay"));

    // Absent extension file yields empty repository baseline (load_or_default).
    fs::remove_file(&repo_agents).expect("remove repo agents");
    fs::remove_file(&overlay_agents).expect("remove overlay agents");
    let absent_agents =
        resolve_config_file(&linked_request, "agents.toml").expect("absent agents ok");
    assert!(absent_agents.bytes.is_empty());
    assert_eq!(
        absent_agents.provenance.winning_layer,
        ConfigLayer::Repository
    );
    fs::write(&repo_agents, "name = \"repo\"\n").expect("restore repo agents");
    fs::write(&overlay_agents, "name = \"overlay\"\n").expect("restore overlay agents");

    // Repository-only owner never consults overlay (publish manifest).
    let repo_manifest = main
        .join(".libra")
        .join("publish")
        .join("worker-template-manifest.json");
    fs::create_dir_all(repo_manifest.parent().unwrap()).expect("publish dir");
    fs::write(&repo_manifest, "{\"v\":1}\n").expect("repo manifest");
    let linked_manifest = linked_gitdir
        .join("publish")
        .join("worker-template-manifest.json");
    fs::create_dir_all(linked_manifest.parent().unwrap()).expect("linked publish dir");
    fs::write(&linked_manifest, "{\"v\":99}\n").expect("linked manifest");
    let manifest = resolve_config_file(&linked_request, "publish/worker-template-manifest.json")
        .expect("manifest");
    assert_eq!(manifest.provenance.owner, ConfigOwner::Repository);
    assert!(manifest.provenance.overlay_path.is_none());
    assert_eq!(manifest.provenance.winning_layer, ConfigLayer::Repository);
    assert!(String::from_utf8_lossy(&manifest.bytes).contains("\"v\":1"));

    // Directory surfaces resolve paths for W4-11/W4-12.
    let rules_dir = main.join(".libra").join("rules");
    fs::create_dir_all(&rules_dir).expect("rules dir");
    let overlay_rules = linked_gitdir.join("rules");
    fs::create_dir_all(&overlay_rules).expect("overlay rules");
    let dir = resolve_config_dir(&linked_request, "rules").expect("rules dir");
    assert_eq!(dir.repository_path, rules_dir);
    assert_eq!(dir.provenance.consumer, ConfigConsumerKind::Security);
    assert_eq!(
        dir.provenance.winning_layer,
        ConfigLayer::RepositoryWithTighteningOverlay
    );

    // Absent security directory repository layer is allowed (empty default).
    fs::remove_dir_all(&rules_dir).expect("remove repository rules");
    let absent_rules = resolve_config_dir(&linked_request, "rules").expect("absent rules ok");
    assert_eq!(
        absent_rules.provenance.winning_layer,
        ConfigLayer::RepositoryWithTighteningOverlay
    );
    fs::create_dir_all(&rules_dir).expect("restore rules dir");

    // Security directory that exists but cannot be enumerated must fail-closed
    // (metadata alone is not enough — mode 000 still stats for the owner).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&rules_dir, fs::Permissions::from_mode(0o000))
            .expect("chmod 000 rules");
        match resolve_config_dir(&linked_request, "rules") {
            Err(err) => {
                assert!(err.is_fail_closed_security(), "got {err}");
                assert!(
                    err.to_string().contains("unreadable") && err.to_string().contains("rules"),
                    "got {err}"
                );
            }
            Ok(ok) => panic!(
                "expected unreadable rules fail-closed, got {:?}",
                ok.provenance
            ),
        }
        fs::set_permissions(&rules_dir, fs::Permissions::from_mode(0o755))
            .expect("restore rules perms");
    }

    // Extension overlay unreadable / inaccessible must not silently fall back.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};

        fn assert_overlay_access_error(err: ConfigResolveError, location: &str) {
            match err {
                ConfigResolveError::Paths { message, .. } => {
                    assert!(
                        (message.contains("unreadable") || message.contains("inaccessible"))
                            && message.contains(location),
                        "got {message}"
                    );
                }
                other => panic!("expected Paths for overlay access error, got {other:?}"),
            }
        }

        fs::set_permissions(&overlay_agents, fs::Permissions::from_mode(0o000))
            .expect("chmod 000 overlay agents");
        assert_overlay_access_error(
            resolve_config_file(&linked_request, "agents.toml")
                .expect_err("extension overlay chmod 000"),
            "agents.toml",
        );
        fs::set_permissions(&overlay_agents, fs::Permissions::from_mode(0o644))
            .expect("restore overlay agents");

        // Symlink into a chmod-000 directory: Path::is_file would look like "missing".
        let trap = parent.path().join("trap-ext");
        fs::create_dir(&trap).expect("trap-ext");
        let trap_agents = trap.join("agents.toml");
        fs::write(&trap_agents, "name = \"trap\"\n").expect("trap agents");
        fs::set_permissions(&trap, fs::Permissions::from_mode(0o000)).expect("chmod trap-ext");
        fs::remove_file(&overlay_agents).expect("remove overlay agents");
        symlink(&trap_agents, &overlay_agents).expect("symlink overlay agents");
        assert_overlay_access_error(
            resolve_config_file(&linked_request, "agents.toml")
                .expect_err("extension overlay via inaccessible parent"),
            "agents.toml",
        );
        fs::set_permissions(&trap, fs::Permissions::from_mode(0o755)).expect("unlock trap-ext");
        fs::remove_file(&overlay_agents).expect("remove symlink agents");
        fs::write(&overlay_agents, "name = \"overlay\"\n").expect("restore overlay agents");

        // Security overlay through inaccessible parent must not drop to repository-only.
        fs::write(&repo_sandbox, "[sandbox.network]\nmode = \"deny\"\n")
            .expect("ensure repository sandbox");
        let trap_sec = parent.path().join("trap-sec");
        fs::create_dir(&trap_sec).expect("trap-sec");
        let trap_sandbox = trap_sec.join("sandbox.toml");
        fs::write(&trap_sandbox, "[sandbox.network]\nmode = \"full\"\n").expect("trap sandbox");
        fs::set_permissions(&trap_sec, fs::Permissions::from_mode(0o000)).expect("chmod trap-sec");
        let _ = fs::remove_file(&overlay_sandbox);
        symlink(&trap_sandbox, &overlay_sandbox).expect("symlink overlay sandbox");
        assert_overlay_access_error(
            resolve_config_file(&linked_request, "sandbox.toml")
                .expect_err("security overlay via inaccessible parent"),
            "sandbox.toml",
        );
        fs::set_permissions(&trap_sec, fs::Permissions::from_mode(0o755)).expect("unlock trap-sec");
        fs::remove_file(&overlay_sandbox).expect("remove sandbox symlink");

        // Security directory overlay through inaccessible parent must not look absent.
        // Symlink *to* a mode-000 dir is insufficient (owner can still stat it); symlink
        // *through* a mode-000 parent forces PermissionDenied on metadata follow.
        let trap_rules_parent = parent.path().join("trap-rules-parent");
        let trap_rules_inner = trap_rules_parent.join("rules");
        fs::create_dir_all(&trap_rules_inner).expect("trap-rules-inner");
        fs::set_permissions(&trap_rules_parent, fs::Permissions::from_mode(0o000))
            .expect("chmod trap-rules-parent");
        let _ = fs::remove_dir_all(&overlay_rules);
        let _ = fs::remove_file(&overlay_rules);
        symlink(&trap_rules_inner, &overlay_rules).expect("symlink overlay rules");
        assert_overlay_access_error(
            resolve_config_dir(&linked_request, "rules")
                .expect_err("security rules overlay via inaccessible parent"),
            "rules",
        );
        fs::set_permissions(&trap_rules_parent, fs::Permissions::from_mode(0o755))
            .expect("unlock trap-rules-parent");
        fs::remove_file(&overlay_rules).expect("remove rules symlink");
        fs::create_dir_all(&overlay_rules).expect("restore overlay rules");

        // Wrong-type overlay (directory where a file is required) must not fall back.
        fs::remove_file(&overlay_agents).expect("remove agents file");
        fs::create_dir(&overlay_agents).expect("agents.toml as directory");
        match resolve_config_file(&linked_request, "agents.toml") {
            Err(ConfigResolveError::Paths { message, .. }) => {
                assert!(
                    message.contains("wrong type") && message.contains("agents.toml"),
                    "got {message}"
                );
            }
            other => panic!("expected wrong-type overlay error, got {other:?}"),
        }
        fs::remove_dir(&overlay_agents).expect("remove wrong-type agents dir");
        fs::write(&overlay_agents, "name = \"overlay\"\n").expect("restore overlay agents");
    }

    // Forged RequestScope (scope key disagrees with pinned gitdir) is rejected.
    let mut forged = linked_request.clone();
    forged.scope = WorktreeScope::Main;
    let mismatch = resolve_config_file(&forged, "agents.toml");
    assert!(matches!(
        mismatch,
        Err(ConfigResolveError::ScopeMismatch { .. })
    ));

    // Forged storage (gitdir A + storage B) must not mix repository layers.
    let other_for_forge = tempfile::tempdir().expect("forge other");
    assert_cli_success(
        &run_libra_command(&["init", "--vault=false"], other_for_forge.path()),
        "init forge other",
    );
    let other_request = request_for(other_for_forge.path());
    let mut mixed = main_request.clone();
    mixed.storage = other_request.storage.clone();
    match resolve_config_file(&mixed, "sandbox.toml") {
        Err(ConfigResolveError::Paths { message, .. }) => {
            assert!(
                message.contains("does not match common storage"),
                "got {message}"
            );
        }
        other => panic!("expected mixed-storage rejection, got {other:?}"),
    }

    // Pinned RequestScope must not follow a later cwd move into another Main repo.
    let other = tempfile::tempdir().expect("other repo");
    assert_cli_success(
        &run_libra_command(&["init", "--vault=false"], other.path()),
        "init other",
    );
    fs::write(
        other.path().join(".libra").join("sandbox.toml"),
        "[sandbox.network]\nmode = \"full\"\n",
    )
    .expect("other sandbox");
    let _cwd = ChangeDirGuard::new(other.path());
    let still_main = resolve_config_file(&main_request, "sandbox.toml").expect("pinned main");
    assert_eq!(
        still_main.provenance.repository_path,
        main_request.storage.join("sandbox.toml")
    );
    assert!(
        String::from_utf8_lossy(&still_main.bytes).contains("deny"),
        "must keep repository A policy after cwd moved to B"
    );

    assert_eq!(
        surface_by_location("config.toml").expect("config").consumer,
        ConfigConsumerKind::Security
    );
    assert_eq!(
        surface_by_location("agents.toml").expect("agents").consumer,
        ConfigConsumerKind::Extension
    );
}

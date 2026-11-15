//! Discovery, lifecycle, compatibility, permissions and failure handling,
//! with tiny hand-written WebAssembly plugins and the sample `snippets`.

use super::*;
use serde_json::json;
use versions::Track;

/// A scratch directory, removed afterwards.
pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("rune-plugins-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const RESERVED: &[&str] = &["save", "mute", "attach"];

/// A plugin that answers every event with `reply`, and from call number
/// `after + 1` on does `misbehave` (WAT instructions) instead.
pub(crate) fn wat_plugin(reply: &str, after: u32, misbehave: &str) -> Vec<u8> {
    wat_module(
        reply,
        after,
        misbehave,
        "(i32.const 65536)",
        "",
        "(memory (export \"memory\") 1)",
    )
}

fn wat_module(reply: &str, after: u32, misbehave: &str, version: &str, extra: &str, memory: &str) -> Vec<u8> {
    let escaped = reply.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"(module
            {extra}
            {memory}
            (data (i32.const 0) "{escaped}")
            (global $n (mut i32) (i32.const 0))
            (func (export "rune_api_version") (result i32) {version})
            (func (export "rune_alloc") (param i32) (result i32) (i32.const 32768))
            (func (export "rune_call") (param i32 i32) (result i64)
                (global.set $n (i32.add (global.get $n) (i32.const 1)))
                (if (i32.gt_u (global.get $n) (i32.const {after})) (then {misbehave}))
                (i64.const {len})))"#,
        len = reply.len()
    );
    wat::parse_str(src).unwrap()
}

pub(crate) const QUIET: &str = r#"{"actions":[]}"#;

pub(crate) fn install(dir: &Path, id: &str, extra: &str, wasm: &[u8]) -> PathBuf {
    let d = dir.join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("plugin.json"),
        format!(r#"{{"id":"{id}","name":"{id}","version":"1","api_version":"1.0"{extra}}}"#),
    )
    .unwrap();
    std::fs::write(d.join("plugin.wasm"), wasm).unwrap();
    d
}

pub(crate) fn snippets(dir: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/snippets");
    let d = dir.join("snippets");
    std::fs::create_dir_all(&d).unwrap();
    for f in ["plugin.json", "snippets.wasm"] {
        std::fs::copy(src.join(f), d.join(f)).unwrap();
    }
}

fn plugins(store: &Store, dir: &TempDir) -> Plugins {
    Plugins::new(store.clone(), Some(dir.0.clone()), false, RESERVED).with_limits(Limits {
        fuel: 20_000_000,
        ..Limits::default()
    })
}

/// Enable a plugin and load it synchronously.
fn enable(p: &mut Plugins, id: &str) -> Vec<Effect> {
    let job = p.enable(id).unwrap();
    p.loaded(job.run())
}

/// Start whatever a scan decided to start.
fn scan(p: &mut Plugins) -> Vec<Effect> {
    let jobs = p.scan();
    jobs.into_iter().flat_map(|j| p.loaded(j.run())).collect()
}

fn state(p: &Plugins, id: &str) -> State {
    p.get(id).unwrap().state.clone()
}

fn statuses(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Status(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn discovery_reports_each_directory_without_running_anything() {
    let dir = TempDir::new("discover");
    let ok = wat_plugin(QUIET, u32::MAX, "");
    install(&dir.0, "good", "", &ok);
    install(&dir.0, "future", "", &ok);
    std::fs::write(
        dir.0.join("future/plugin.json"),
        r#"{"id":"future","name":"F","version":"1","api_version":"2.0"}"#,
    )
    .unwrap();
    // A manifest naming another id, one without a module, one unparsable.
    install(&dir.0, "renamed", "", &ok);
    std::fs::write(
        dir.0.join("renamed/plugin.json"),
        r#"{"id":"other","name":"x","version":"1","api_version":"1.0"}"#,
    )
    .unwrap();
    install(&dir.0, "nomodule", "", &ok);
    std::fs::remove_file(dir.0.join("nomodule/plugin.wasm")).unwrap();
    install(&dir.0, "broken", "", &ok);
    std::fs::write(dir.0.join("broken/plugin.json"), "{ not json").unwrap();
    std::fs::create_dir_all(dir.0.join("Not_An_Id")).unwrap();
    std::fs::create_dir_all(dir.0.join(".hidden")).unwrap();
    std::fs::write(dir.0.join("stray-file"), "x").unwrap();

    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    assert!(p.scan().is_empty(), "nothing is enabled, so nothing starts");
    let ids: Vec<&str> = p.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        ["Not_An_Id", "broken", "future", "good", "nomodule", "renamed"]
    );
    assert_eq!(state(&p, "good"), State::Disabled);
    assert!(matches!(state(&p, "future"), State::Incompatible(s) if s.contains("2.0")));
    for (id, why) in [
        ("renamed", "says id"),
        ("nomodule", "missing"),
        ("broken", "plugin.json"),
        ("Not_An_Id", "not a plugin id"),
    ] {
        assert!(
            matches!(state(&p, id), State::Invalid(s) if s.contains(why)),
            "{id}: {:?}",
            state(&p, id)
        );
    }
    assert!(p.enable("broken").is_err() && p.enable("future").is_err());
}

#[cfg(unix)]
#[test]
fn modules_other_users_could_replace_are_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("perm");
    let d = install(&dir.0, "shared", "", &wat_plugin(QUIET, u32::MAX, ""));
    std::fs::set_permissions(d.join("plugin.wasm"), std::fs::Permissions::from_mode(0o666)).unwrap();
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    assert!(matches!(state(&p, "shared"), State::Invalid(s) if s.contains("writable by other users")));
}

#[test]
fn the_sample_plugin_adds_a_working_feature() {
    let dir = TempDir::new("snippets");
    snippets(&dir.0);
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    scan(&mut p);
    assert_eq!(state(&p, "snippets"), State::Disabled);
    assert_eq!(
        p.command("snip", "", None),
        Some(vec![Effect::Status(
            "/snip comes from the plugin Snippets, which is off (see Ctrl-x › Plugins)".into()
        )])
    );

    let fx = enable(&mut p, "snippets");
    assert_eq!(state(&p, "snippets"), State::Active, "{fx:?}");
    assert_eq!(p.commands_help()[0].0, "/snip [NAME | add [NAME] | rm NAME]");
    assert_eq!(p.keys_help()[0].0, "Alt-s");

    // /snip add → a form; submitting it stores the snippet and shows the list.
    let fx = p.command("snip", "add greet", None).unwrap();
    assert!(
        matches!(&fx[..], [Effect::Form { id, fields, .. }] if id == "add" && fields[0].value == "greet")
    );
    let fx = p.deliver(
        "snippets",
        Event::FormSubmitted {
            form: "add".into(),
            values: vec!["greet".into(), "Good morning!".into()],
        },
    );
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::OpenView { interactive: true, view } if view.lines.len() == 1))
    );
    assert_eq!(p.data("snippets").len(), 1);
    // /snip NAME types it into the message line.
    let fx = p.command("snip", "greet", None).unwrap();
    assert!(fx.contains(&Effect::InsertText("Good morning!".into())), "{fx:?}");
    // Alt-s opens the list.
    let fx = p.key('s', None).unwrap();
    assert!(matches!(&fx[..], [Effect::OpenView { view, .. }] if view.title == "Snippets (1)"));
    assert!(p.key('x', None).is_none());

    // A setting change reaches the running plugin.
    assert!(p.set_setting("snippets", "preview", json!(20)).is_ok());
    assert!(p.set_setting("snippets", "preview", json!(500)).is_err());
    assert_eq!(p.config("snippets")["preview"], 20);

    // Started again next run (enabled and approved), with its data.
    let mut again = plugins(&store, &dir);
    scan(&mut again);
    assert_eq!(state(&again, "snippets"), State::Active);
    let fx = again.command("snip", "greet", None).unwrap();
    assert!(fx.contains(&Effect::InsertText("Good morning!".into())));

    // Disabled: off now and next run; the command says where it went.
    let fx = again.disable("snippets");
    assert!(fx.contains(&Effect::CloseView {
        plugin: "snippets".into(),
        id: None
    }));
    assert_eq!(state(&again, "snippets"), State::Disabled);
    assert!(again.key('s', None).is_none());
    let mut third = plugins(&store, &dir);
    assert!(third.scan().is_empty());
}

#[test]
fn a_changed_module_or_permission_list_needs_approving_again() {
    let dir = TempDir::new("approve");
    let d = install(&dir.0, "p", "", &wat_plugin(QUIET, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "p");
    assert_eq!(state(&p, "p"), State::Active);

    // New permissions: not started until approved.
    std::fs::write(
        d.join("plugin.json"),
        r#"{"id":"p","name":"p","version":"2","api_version":"1.0","permissions":["send_messages"]}"#,
    )
    .unwrap();
    let mut next = plugins(&store, &dir);
    assert!(next.scan().is_empty());
    assert!(matches!(state(&next, "p"), State::NeedsApproval(_)));
    enable(&mut next, "p");
    assert_eq!(state(&next, "p"), State::Active);

    // A new module: the same. Restart (as after rebuilding) notices too.
    std::fs::write(
        d.join("plugin.wasm"),
        wat_plugin(r#"{"actions": []}"#, u32::MAX, ""),
    )
    .unwrap();
    let (_, job) = next.restart("p");
    assert!(job.is_none());
    assert!(matches!(state(&next, "p"), State::NeedsApproval(_)));

    // Swapped between approval and compilation: refused as well.
    let job = next.enable("p").unwrap();
    std::fs::write(d.join("plugin.wasm"), wat_plugin(QUIET, 0, "unreachable")).unwrap();
    next.loaded(job.run());
    assert!(matches!(state(&next, "p"), State::NeedsApproval(s) if s.contains("changed on disk")));
}

#[test]
fn no_plugins_mode_starts_nothing() {
    let dir = TempDir::new("safe");
    install(&dir.0, "p", "", &wat_plugin(QUIET, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    enable(&mut plugins(&store, &dir).tap_scan(), "p");
    let mut safe = Plugins::new(store.clone(), Some(dir.0.clone()), true, RESERVED);
    assert!(safe.scan().is_empty());
    assert_eq!(state(&safe, "p"), State::Paused);
}

impl Plugins {
    fn tap_scan(mut self) -> Self {
        self.scan();
        self
    }
}

#[test]
fn module_api_versions_are_checked_as_well_as_the_manifest() {
    let dir = TempDir::new("api");
    let v2 = wat_module(
        QUIET,
        u32::MAX,
        "",
        "(i32.const 131072)",
        "",
        "(memory (export \"memory\") 1)",
    );
    install(&dir.0, "liar", "", &v2);
    let v1_1 = wat_module(
        QUIET,
        u32::MAX,
        "",
        "(i32.const 65537)",
        "",
        "(memory (export \"memory\") 1)",
    );
    install(&dir.0, "newer", "", &v1_1);
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "liar");
    assert!(matches!(state(&p, "liar"), State::Incompatible(s) if s.contains("2.0")));
    enable(&mut p, "newer");
    assert!(matches!(state(&p, "newer"), State::Incompatible(s) if s.contains("1.1")));
}

#[test]
fn broken_modules_fail_to_start_and_say_why() {
    let dir = TempDir::new("broken");
    let mem = "(memory (export \"memory\") 1)";
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("garbage", b"\0asm nonsense".to_vec(), "not a valid WebAssembly module"),
        (
            "wants-system",
            wat_module(QUIET, u32::MAX, "", "(i32.const 65536)", r#"(import "env" "system" (func))"#, mem),
            "env.system",
        ),
        (
            "no-call",
            wat::parse_str(r#"(module (memory (export "memory") 1) (func (export "rune_api_version") (result i32) (i32.const 65536)) (func (export "rune_alloc") (param i32) (result i32) (i32.const 0)))"#).unwrap(),
            "rune_call",
        ),
        (
            "huge-memory",
            wat_module(QUIET, u32::MAX, "", "(i32.const 65536)", "", "(memory (export \"memory\") 2000)"),
            "could not start",
        ),
        (
            "loops-at-start",
            wat_module(QUIET, u32::MAX, "", "(i32.const 65536)", "(func $s (loop $l (br $l))) (start $s)", mem),
            "fuel",
        ),
        ("crashes-in-init", wat_plugin(QUIET, 0, "unreachable"), "crashed"),
    ];
    for (id, wasm, _) in &cases {
        install(&dir.0, id, "", wasm);
    }
    install(&dir.0, "zz-fine", "", &wat_plugin(QUIET, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    for (id, _, why) in &cases {
        let fx = enable(&mut p, id);
        assert!(
            matches!(state(&p, id), State::Failed(s) if s.contains(why)),
            "{id}: {:?}",
            state(&p, id)
        );
        assert!(!statuses(&fx).is_empty(), "{id}: the failure is reported");
    }
    enable(&mut p, "zz-fine");
    assert_eq!(
        state(&p, "zz-fine"),
        State::Active,
        "one plugin's failure is not another's"
    );
}

#[test]
fn a_plugin_that_misbehaves_later_is_stopped_and_the_rest_keep_going() {
    let dir = TempDir::new("later");
    let mem = "(memory (export \"memory\") 1)";
    let cmd = r#","commands":[{"name":"go","help":"x"}]"#;
    let cases = [
        ("trap", wat_plugin(QUIET, 1, "unreachable"), "crashed"),
        (
            "spin",
            wat_plugin(QUIET, 1, "(loop $l (br $l))"),
            "ran out of fuel",
        ),
        (
            "wild-pointer",
            wat_plugin(QUIET, 1, "(return (i64.const 0x0000FFFF00000010))"),
            "outside its memory",
        ),
        (
            "huge-reply",
            wat_plugin(QUIET, 1, "(return (i64.const 0x7FFFFFFF))"),
            "more than",
        ),
        (
            "not-json",
            wat_module("nope", u32::MAX, "", "(i32.const 65536)", "", mem),
            "not JSON",
        ),
        (
            "grows-forever",
            wat_plugin(
                QUIET,
                1,
                "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable",
            ),
            "crashed",
        ),
    ];
    for (id, wasm, _) in &cases {
        install(&dir.0, id, cmd, wasm);
    }
    install(
        &dir.0,
        "zz-calm",
        r#","commands":[{"name":"calm","help":"x"}]"#,
        &wat_plugin(r#"{"actions":[{"type":"status","text":"ok"}]}"#, u32::MAX, ""),
    );
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "zz-calm");
    for (id, _, why) in &cases {
        let fx = enable(&mut p, id);
        if *id == "not-json" {
            // Its very first reply (to init) is already unreadable.
            assert!(
                matches!(state(&p, id), State::Failed(s) if s.contains(why)),
                "{id}"
            );
            continue;
        }
        assert_eq!(state(&p, id), State::Active, "{id}: {fx:?}");
        let fx = p.command("go", "", None).unwrap();
        assert!(
            matches!(state(&p, id), State::Failed(s) if s.contains(why)),
            "{id}: {:?}",
            state(&p, id)
        );
        assert!(fx.contains(&Effect::CloseView {
            plugin: id.to_string(),
            id: None
        }));
        assert!(statuses(&fx)[0].contains("stopped"), "{fx:?}");
        // The failed plugin's command is no longer dispatched to it.
        assert!(
            matches!(&p.command("go", "", None).unwrap()[..], [Effect::Status(s)] if s.contains("see Ctrl-x › Plugins"))
        );
        assert_eq!(statuses(&p.command("calm", "", None).unwrap()), ["zz-calm: ok"]);
    }
    // Restart brings a failed plugin back.
    let (_, job) = p.restart("trap");
    p.loaded(job.unwrap().run());
    assert_eq!(state(&p, "trap"), State::Active);
}

/// A reply with one action of every kind.
const EVERYTHING: &str = r#"{"actions":[
    {"type":"status","text":"hi\u001b[31m"},
    {"type":"notify","title":"t","body":"b"},
    {"type":"insert_text","text":"typed"},
    {"type":"send_message","chat":"c1","text":"spam"},
    {"type":"store_set","key":"k","value":"v"},
    {"type":"form","id":"f","title":"F","fields":[{"label":"L"}]},
    {"type":"open_view","id":"v","title":"V","lines":[{"text":"x","style":"bold"}],"selected":5},
    {"type":"set_timer","id":"t","after_ms":10},
    {"type":"teleport"}
]}"#;

fn everything(dir: &TempDir, store: &Store, perms: &str) -> Plugins {
    let extra = format!(r#","permissions":[{perms}],"commands":[{{"name":"go","help":"x"}}]"#);
    // Quiet for init, then the full reply.
    let wasm = wat_two_replies(QUIET, EVERYTHING);
    install(&dir.0, "all", &extra, &wasm);
    let mut p = plugins(store, dir);
    p.scan();
    enable(&mut p, "all");
    assert_eq!(state(&p, "all"), State::Active);
    p
}

/// `first` for the first call (init), `then` for every later one.
fn wat_two_replies(first: &str, then: &str) -> Vec<u8> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ");
    let then = then.replace('\n', " ");
    let src = format!(
        r#"(module (memory (export "memory") 1)
            (data (i32.const 0) "{a}")
            (data (i32.const 4096) "{b}")
            (global $n (mut i32) (i32.const 0))
            (func (export "rune_api_version") (result i32) (i32.const 65536))
            (func (export "rune_alloc") (param i32) (result i32) (i32.const 32768))
            (func (export "rune_call") (param i32 i32) (result i64)
                (global.set $n (i32.add (global.get $n) (i32.const 1)))
                (if (i32.gt_u (global.get $n) (i32.const 1))
                    (then (return (i64.or (i64.shl (i64.const 4096) (i64.const 32)) (i64.const {blen})))))
                (i64.const {alen})))"#,
        a = esc(first),
        b = esc(&then),
        alen = first.len(),
        blen = then.len()
    );
    wat::parse_str(src).unwrap()
}

#[test]
fn actions_need_their_permissions() {
    let dir = TempDir::new("noperm");
    let store = Store::open_in_memory().unwrap();
    let mut p = everything(&dir, &store, "");
    let fx = p.command("go", "", None).unwrap();
    let s = statuses(&fx);
    assert_eq!(
        s[0], "all: hi\u{FFFD}[31m",
        "control characters never reach the terminal"
    );
    for refused in ["notify", "compose", "send_messages", "storage"] {
        assert!(
            s.iter().any(|m| m.contains(refused) && m.contains("refused")),
            "{refused}: {s:?}"
        );
    }
    assert!(
        s.iter().any(|m| m.contains("does not know")),
        "unknown actions are refused: {s:?}"
    );
    assert!(!fx.iter().any(|e| matches!(
        e,
        Effect::Notify { .. } | Effect::InsertText(_) | Effect::SendMessage { .. }
    )));
    assert!(p.data("all").is_empty());
    // Views, forms and timers need no permission.
    assert!(fx.iter().any(|e| matches!(e, Effect::Form { .. })));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::OpenView { view, interactive: true } if view.selected.is_none()))
    );
    assert!(p.next_timer().is_some());
    assert_eq!(state(&p, "all"), State::Active, "refusals do not stop a plugin");
}

#[test]
fn granted_actions_go_through_and_background_events_cannot_take_the_keyboard() {
    let dir = TempDir::new("perm");
    let store = Store::open_in_memory().unwrap();
    let mut p = everything(
        &dir,
        &store,
        r#""notify","compose","send_messages","storage","read_messages""#,
    );
    let fx = p.command("go", "", None).unwrap();
    assert!(fx.contains(&Effect::InsertText("typed".into())));
    assert!(fx.contains(&Effect::Notify {
        title: "t".into(),
        body: "b".into()
    }));
    assert!(fx.contains(&Effect::SendMessage {
        plugin: "all".into(),
        chat: "c1".into(),
        text: "spam".into()
    }));
    assert_eq!(p.data("all").get("k").map(String::as_str), Some("v"));

    // The same reply to a message (not something the user did): no form,
    // no typing, and the view may only update an open one.
    let chat = ChatRef {
        mid: "c1".into(),
        name: "C".into(),
        kind: ChatKind::User,
    };
    let msg = MessageInfo {
        id: "1".into(),
        from: "u".into(),
        from_name: "U".into(),
        text: Some("x".into()),
        summary: "x".into(),
        time: 0,
        mine: false,
    };
    let fx = p.message(chat, msg);
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::Form { .. } | Effect::InsertText(_))),
        "{fx:?}"
    );
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::OpenView {
            interactive: false,
            ..
        }
    )));
    assert!(
        statuses(&fx)
            .iter()
            .any(|s| s.contains("only answers something you did"))
    );

    // Sending is limited to a few messages a minute.
    let sent = |fx: &[Effect]| {
        fx.iter()
            .filter(|e| matches!(e, Effect::SendMessage { .. }))
            .count()
    };
    let total: usize = (0..10).map(|_| sent(&p.command("go", "", None).unwrap())).sum();
    assert_eq!(total + 2, SENDS_PER_MINUTE, "two were sent above");
}

#[test]
fn timers_fire_and_repeat_only_while_the_plugin_runs() {
    let dir = TempDir::new("timer");
    let reply = r#"{"actions":[{"type":"set_timer","id":"tick","after_ms":1000,"repeat":true}]}"#;
    install(&dir.0, "clock", "", &wat_plugin(reply, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "clock");
    let at = p.next_timer().expect("init set a timer");
    assert!(p.run_timers(Instant::now()).is_empty(), "not due yet");
    p.run_timers(at + Duration::from_millis(1));
    assert!(p.next_timer().unwrap() > at, "a repeating timer comes back");
    p.disable("clock");
    assert_eq!(p.next_timer(), None);
}

/// Regression: timer ids are cleaned when stored, and used to be compared
/// uncleaned, so an id with a control character (or over 64 characters)
/// could be neither replaced nor cancelled.
#[test]
fn timer_ids_that_need_cleaning_are_replaced_and_cancelled() {
    let dir = TempDir::new("timer-ids");
    let odd = format!("t\\u0001{}", "x".repeat(80));
    let set = format!(r#"{{"type":"set_timer","id":"{odd}","after_ms":5000}}"#);
    let cancel = format!(r#"{{"type":"cancel_timer","id":"{odd}"}}"#);
    let twice = format!(r#"{{"actions":[{set},{set}]}}"#);
    let set_then_cancel = format!(r#"{{"actions":[{set},{cancel}]}}"#);
    install(&dir.0, "twice", "", &wat_plugin(&twice, u32::MAX, ""));
    install(&dir.0, "undo", "", &wat_plugin(&set_then_cancel, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "twice");
    enable(&mut p, "undo");
    assert_eq!(
        p.get("twice").unwrap().timers.len(),
        1,
        "setting it again replaces it"
    );
    assert_eq!(p.get("undo").unwrap().timers.len(), 0, "cancelling finds it");
}

#[test]
fn rune_commands_stay_runes() {
    let dir = TempDir::new("reserved");
    install(
        &dir.0,
        "greedy",
        r#","commands":[{"name":"save","help":"x"},{"name":"mine","help":"y"}]"#,
        &wat_plugin(QUIET, u32::MAX, ""),
    );
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    let fx = enable(&mut p, "greedy");
    assert!(statuses(&fx).iter().any(|s| s.contains("/save is rune's own")));
    assert_eq!(p.command("save", "", None), None);
    assert!(p.command("mine", "", None).is_some());
    assert_eq!(p.commands_help().len(), 1);
}

#[test]
fn settings_are_validated_persisted_and_fall_back_to_defaults() {
    let dir = TempDir::new("settings");
    let extra = r#","settings":[{"key":"n","label":"N","type":"int","default":3,"min":1,"max":5},{"key":"on","label":"On","type":"bool","default":true}]"#;
    install(&dir.0, "cfg", extra, &wat_plugin(QUIET, u32::MAX, ""));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    assert_eq!(
        p.config("cfg"),
        json!({"n": 3, "on": true}).as_object().unwrap().clone()
    );
    assert!(
        p.set_setting("cfg", "n", json!("4")).is_ok(),
        "typed numbers are accepted"
    );
    assert_eq!(
        p.set_setting("cfg", "n", json!(9)),
        Err("must be at most 5".into())
    );
    assert!(p.set_setting("cfg", "nope", json!(1)).is_err());
    assert_eq!(p.config("cfg")["n"], 4);
    // Garbage in the store (an older version's value) gives the default.
    store.set("setting.plugin.cfg.config.on", "\"maybe\"").unwrap();
    assert_eq!(p.config("cfg")["on"], true);
    p.reset_settings("cfg").unwrap();
    assert_eq!(p.config("cfg")["n"], 3);
}

#[test]
fn removed_plugins_leave_an_entry_that_can_be_forgotten() {
    let dir = TempDir::new("missing");
    let store = Store::open_in_memory().unwrap();
    store.set("setting.plugin.ghost.enabled", "true").unwrap();
    store.set("plugin.ghost.data.k", "v").unwrap();
    store.set("plugin.ghostly.data.k", "v").unwrap();
    let mut p = plugins(&store, &dir);
    assert!(p.scan().is_empty());
    assert_eq!(state(&p, "ghost"), State::Missing);
    assert!(p.enable("ghost").is_err());
    p.forget("ghost");
    assert!(p.get("ghost").is_none());
    assert!(
        store
            .values_with_prefix("setting.plugin.ghost.")
            .unwrap()
            .is_empty()
    );
    assert!(store.values_with_prefix("plugin.ghost.").unwrap().is_empty());
    assert_eq!(
        p.data("ghostly").len(),
        1,
        "a plugin whose id starts the same is untouched"
    );
}

#[test]
fn clean_strips_escapes_and_cuts_long_text() {
    assert_eq!(clean("a\x1b[2Jb\u{9b}c", 50, false), "a\u{FFFD}[2Jb\u{FFFD}c");
    assert_eq!(clean("line\nbreak\r\n", 50, true), "line\nbreak\n");
    assert_eq!(clean("line\nbreak", 50, false), "line\u{FFFD}break");
    assert_eq!(clean("abcdef", 4, false), "abc…");
    assert_eq!(clean("abcd", 4, false), "abcd");
}

// -- hot reload ----------------------------------------------------------------------

/// A module that says `text` on every event.
pub(crate) fn says(text: &str) -> Vec<u8> {
    wat_plugin(
        &format!(r#"{{"actions":[{{"type":"status","text":"{text}"}}]}}"#),
        u32::MAX,
        "",
    )
}

fn refresh(p: &mut Plugins, id: &str) -> (Vec<String>, Vec<LoadJob>) {
    let (fx, jobs) = p.refresh(&[id.to_owned()]);
    (statuses(&fx), jobs)
}

/// Load what a refresh started.
fn finish(p: &mut Plugins, jobs: Vec<LoadJob>) -> Vec<String> {
    jobs.into_iter()
        .flat_map(|j| statuses(&p.loaded(j.run())))
        .collect()
}

#[test]
fn changed_files_reload_a_running_plugin_without_ever_running_unapproved_code() {
    let dir = TempDir::new("hot");
    let d = install(&dir.0, "hot", "", &says("v1"));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    assert_eq!(statuses(&enable(&mut p, "hot")), ["hot: v1"]);

    // Looking again at unchanged files does nothing.
    assert_eq!(refresh(&mut p, "hot").0, Vec::<String>::new());

    // A rebuilt module waits for approval; the old one keeps running.
    std::fs::write(d.join("plugin.wasm"), says("v2")).unwrap();
    let (s, jobs) = refresh(&mut p, "hot");
    assert!(
        jobs.is_empty() && s[0].contains("approve the new version"),
        "{s:?}"
    );
    assert_eq!(state(&p, "hot"), State::Active);
    assert!(
        matches!(p.get("hot").unwrap().update(), Some(Update::Approval { new_permissions, .. }) if new_permissions.is_empty())
    );
    assert_eq!(
        statuses(&p.deliver("hot", Event::Timer { id: "t".into() })),
        ["hot: v1"]
    );
    assert!(refresh(&mut p, "hot").0.is_empty(), "reported once");

    // Approved: loaded beside the old one, then swapped in.
    let job = p.approve_update("hot").unwrap();
    assert_eq!(state(&p, "hot"), State::Active, "still the old one while loading");
    let s = statuses(&p.loaded(job.run()));
    assert_eq!(
        s,
        ["hot: v1", "reloaded hot 1", "hot: v2"],
        "the old one's shutdown, the new one's init"
    );
    assert!(p.get("hot").unwrap().update().is_none());

    // With "load changed versions automatically", a rebuild just reloads…
    assert!(p.set_autoload("hot", true).unwrap().is_none());
    std::fs::write(d.join("plugin.wasm"), says("v3")).unwrap();
    let (s, jobs) = refresh(&mut p, "hot");
    assert_eq!(s, ["reloading hot…"]);
    assert_eq!(finish(&mut p, jobs), ["hot: v2", "reloaded hot 1", "hot: v3"]);
    // …and the next run starts it without asking too.
    let mut next = plugins(&store, &dir);
    assert_eq!(statuses(&scan(&mut next)), ["hot: v3"]);
    assert_eq!(state(&next, "hot"), State::Active);
    drop(next);

    // …but never with a new permission.
    std::fs::write(
        d.join("plugin.json"),
        r#"{"id":"hot","name":"hot","version":"4","api_version":"1.0","permissions":["notify"]}"#,
    )
    .unwrap();
    let (s, jobs) = refresh(&mut p, "hot");
    assert!(jobs.is_empty() && s[0].contains("approve"), "{s:?}");
    assert!(matches!(
        p.get("hot").unwrap().update(),
        Some(Update::Approval { new_permissions, .. }) if new_permissions == &[Permission::Notify]
    ));

    // Unusable files, or a module that does not start: the old one stays.
    std::fs::write(d.join("plugin.json"), "{ half-written").unwrap();
    let (s, jobs) = refresh(&mut p, "hot");
    assert!(jobs.is_empty() && s[0].contains("not usable"), "{s:?}");
    assert!(matches!(p.get("hot").unwrap().update(), Some(Update::Broken(_))));
    std::fs::write(
        d.join("plugin.json"),
        r#"{"id":"hot","name":"hot","version":"5","api_version":"1.0"}"#,
    )
    .unwrap();
    let mem = "(memory (export \"memory\") 1)";
    let start_traps = wat_module(
        QUIET,
        u32::MAX,
        "",
        "(i32.const 65536)",
        "(func $s unreachable) (start $s)",
        mem,
    );
    let api_2 = wat_module(QUIET, u32::MAX, "", "(i32.const 131072)", "", mem);
    for (module, why) in [(start_traps, "does not start"), (api_2, "speaks plugin API 2.0")] {
        std::fs::write(d.join("plugin.wasm"), module).unwrap();
        let (_, jobs) = refresh(&mut p, "hot");
        let s = finish(&mut p, jobs);
        assert!(s[0].contains(why) && s[0].contains("running one stays"), "{s:?}");
        assert_eq!(state(&p, "hot"), State::Active);
        assert!(matches!(p.get("hot").unwrap().update(), Some(Update::Broken(b)) if b.contains(why)));
        assert_eq!(
            statuses(&p.deliver("hot", Event::Timer { id: "t".into() })),
            ["hot: v3"]
        );
    }

    // Disabled with a newer version waiting: the page shows the one on disk.
    p.disable("hot");
    assert_eq!(p.get("hot").unwrap().manifest.as_ref().unwrap().version, "5");
    assert!(p.get("hot").unwrap().update().is_none());

    // A new version that loads but crashes in `init` is stopped like any
    // crash (the previous one has shut down by then); fixed files retry it.
    enable(&mut p, "hot");
    std::fs::write(d.join("plugin.wasm"), says("v6")).unwrap();
    let (_, jobs) = refresh(&mut p, "hot");
    finish(&mut p, jobs);
    std::fs::write(d.join("plugin.wasm"), wat_plugin(QUIET, 0, "unreachable")).unwrap();
    let (_, jobs) = refresh(&mut p, "hot");
    let s = finish(&mut p, jobs);
    assert!(s.last().unwrap().contains("plugin hot stopped: crashed"), "{s:?}");
    assert!(matches!(state(&p, "hot"), State::Failed(_)));
    std::fs::write(d.join("plugin.wasm"), says("v7")).unwrap();
    let (_, jobs) = refresh(&mut p, "hot");
    assert_eq!(finish(&mut p, jobs), ["hot: v7"]);
}

#[test]
fn a_failed_plugin_retries_with_new_files_and_plugins_come_and_go() {
    let dir = TempDir::new("hot2");
    let d = install(&dir.0, "flaky", "", &wat_plugin(QUIET, 1, "unreachable"));
    let store = Store::open_in_memory().unwrap();
    let mut p = plugins(&store, &dir);
    p.scan();
    enable(&mut p, "flaky");
    p.set_autoload("flaky", true).unwrap();
    p.deliver("flaky", Event::Timer { id: "t".into() });
    assert!(matches!(state(&p, "flaky"), State::Failed(_)));
    // Fixed and rebuilt: tried again by itself.
    std::fs::write(d.join("plugin.wasm"), says("fixed")).unwrap();
    let (s, jobs) = refresh(&mut p, "flaky");
    assert_eq!(s, ["reloading flaky…"]);
    assert_eq!(finish(&mut p, jobs), ["flaky: fixed"]);
    assert_eq!(state(&p, "flaky"), State::Active);

    // Removed while running: stopped; its settings keep an entry.
    std::fs::remove_dir_all(&d).unwrap();
    let (fx, _) = p.refresh(&["flaky".into()]);
    assert!(fx.iter().any(|e| matches!(e, Effect::CloseView { .. })));
    assert!(
        statuses(&fx)
            .iter()
            .any(|s| s.contains("removed from disk; stopped"))
    );
    assert_eq!(state(&p, "flaky"), State::Missing);
    // Back again, still enabled and approved for this module: it starts.
    install(&dir.0, "flaky", "", &says("fixed"));
    let (s, jobs) = refresh(&mut p, "flaky");
    assert_eq!(s, ["starting flaky…"]);
    assert_eq!(finish(&mut p, jobs), ["flaky: fixed"]);

    // A new plugin appears (off); one that was never used disappears for good.
    install(&dir.0, "fresh", "", &says("hi"));
    let (s, jobs) = refresh(&mut p, "fresh");
    assert!(
        jobs.is_empty() && s[0].starts_with("found plugin fresh 1: enable it"),
        "{s:?}"
    );
    assert_eq!(state(&p, "fresh"), State::Disabled);
    std::fs::remove_dir_all(dir.0.join("fresh")).unwrap();
    refresh(&mut p, "fresh");
    assert!(p.get("fresh").is_none());
    // Hidden or odd names are never looked at.
    assert!(p.refresh(&[".staging".into(), "../x".into()]).0.is_empty());
}

// -- installing from Git -------------------------------------------------------------

pub(crate) fn have_git() -> bool {
    let ok = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("git is not installed: skipping");
    }
    ok
}

pub(crate) fn git_in(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository holding `files` (path, bytes), committed.
pub(crate) fn repo_with(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q"]);
    commit(&repo, files, "first");
    repo
}

pub(crate) fn commit(repo: &Path, files: &[(&str, &[u8])], msg: &str) {
    for (path, bytes) in files {
        let p = repo.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    git_in(repo, &["add", "-A"]);
    git_in(repo, &["commit", "-q", "-m", msg]);
}

pub(crate) fn manifest(id: &str, version: &str) -> Vec<u8> {
    format!(r#"{{"id":"{id}","name":"{id}","version":"{version}","api_version":"1.0"}}"#).into_bytes()
}

fn request(repo: &Path, version: &str, subdir: &str) -> install::Request {
    install::Request::new(&repo.display().to_string(), version, subdir).unwrap()
}

#[test]
fn plugins_install_from_a_repository_and_run_only_once_enabled() {
    if !have_git() {
        return;
    }
    let work = TempDir::new("git-src");
    let dir = TempDir::new("git-dst");
    let plugins_dir = dir.0.join("plugins");
    let repo = repo_with(
        &work.0,
        &[
            ("plugin.json", &manifest("fromgit", "1.0")),
            ("plugin.wasm", &says("v1")),
            ("README.md", b"hi"),
            ("src/lib.rs", b"// source stays behind"),
        ],
    );
    let lim = Limits::default();
    let said = std::cell::RefCell::new(Vec::new());
    let progress = |s: String| said.borrow_mut().push(s);
    let o = install::install(&plugins_dir, &request(&repo, "", ""), lim, &progress).unwrap();
    assert_eq!(
        (o.id.as_str(), o.version.as_str(), o.up_to_date),
        ("fromgit", "1.0", false)
    );
    assert!(said.borrow()[0].starts_with("looking at "), "{:?}", said.borrow());

    // Only the manifest, the module and the source record are installed;
    // the staging area is cleaned up.
    let installed = plugins_dir.join("fromgit");
    let mut files: Vec<String> = std::fs::read_dir(&installed)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, [install::SOURCE_FILE, "plugin.json", "plugin.wasm"]);
    let staged = std::fs::read_dir(plugins_dir.join(".staging")).unwrap().count();
    assert_eq!(staged, 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(installed.join("plugin.wasm"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o022, 0, "not writable by others");
    }

    // The host sees it, off, with where it came from.
    let store = Store::open_in_memory().unwrap();
    let mut p = Plugins::new(store.clone(), Some(plugins_dir.clone()), false, RESERVED);
    let (s, jobs) = refresh(&mut p, "fromgit");
    assert!(jobs.is_empty() && s[0].contains("enable it"), "{s:?}");
    let src = p.get("fromgit").unwrap().source.clone().unwrap();
    assert_eq!(src.url, repo.display().to_string());
    assert_eq!(src.commit, o.commit);

    // Installing it again is refused with what to do instead.
    let again = install::install(&plugins_dir, &request(&repo, "", ""), lim, &|_| {}).unwrap_err();
    assert!(again.contains("already installed from there"), "{again}");

    // Up to date, then a new commit: updated in place, and the new module
    // waits for approval beside the running one.
    enable(&mut p, "fromgit");
    let o = install::update(&plugins_dir, "fromgit", None, lim, &|_| {}).unwrap();
    assert!(o.up_to_date);
    commit(
        &repo,
        &[
            ("plugin.json", &manifest("fromgit", "1.1")),
            ("plugin.wasm", &says("v2")),
        ],
        "v2",
    );
    let o = install::update(&plugins_dir, "fromgit", None, lim, &|_| {}).unwrap();
    assert!(!o.up_to_date);
    assert_eq!(o.previous.as_ref().unwrap().0, "1.0");
    assert_eq!(o.version, "1.1");
    let (s, _) = refresh(&mut p, "fromgit");
    assert!(s[0].contains("approve the new version"), "{s:?}");
    let job = p.approve_update("fromgit").unwrap();
    assert_eq!(
        statuses(&p.loaded(job.run())),
        ["fromgit: v1", "reloaded fromgit 1.1", "fromgit: v2"]
    );
    assert_eq!(
        p.get("fromgit").unwrap().source.as_ref().unwrap().commit,
        o.commit
    );

    // Uninstalled: stopped, files gone, settings kept until forgotten.
    store.set("setting.plugin.fromgit.config.x", "1").unwrap();
    p.uninstall("fromgit").unwrap();
    assert!(!installed.exists());
    assert_eq!(state(&p, "fromgit"), State::Missing);
    assert!(store.get("setting.plugin.fromgit.approved").unwrap().is_none());

    // A plugin copied in by hand is never deleted by rune.
    install(&plugins_dir, "byhand", "", &says("x"));
    refresh(&mut p, "byhand");
    assert!(p.uninstall("byhand").unwrap_err().contains("yourself"));
    assert!(plugins_dir.join("byhand/plugin.wasm").exists());
    let clash = repo_with(
        &work.0.join("clash"),
        &[
            ("plugin.json", &manifest("byhand", "1")),
            ("plugin.wasm", &says("y")),
        ],
    );
    let e = install::install(&plugins_dir, &request(&clash, "", ""), lim, &|_| {}).unwrap_err();
    assert!(e.contains("copied into") && e.contains("by hand"), "{e}");
}

#[test]
fn repositories_that_hold_no_usable_plugin_are_refused_and_say_why() {
    if !have_git() {
        return;
    }
    let work = TempDir::new("git-bad");
    let dir = TempDir::new("git-bad-dst");
    let lim = Limits::default();
    let mut n = 0;
    let mut try_install = |files: &[(&str, &[u8])], git_ref: &str, subdir: &str| {
        n += 1;
        let repo = repo_with(&work.0.join(n.to_string()), files);
        install::install(&dir.0, &request(&repo, git_ref, subdir), lim, &|_| {})
    };
    let ok = says("ok");
    let m = |id: &str| manifest(id, "1");
    // Files, branch, folder, the reason given.
    type Case<'a> = (Vec<(&'a str, Vec<u8>)>, &'a str, &'a str, &'a str);
    let cases: Vec<Case> = vec![
        (
            vec![("README.md", b"no plugin".to_vec())],
            "",
            "",
            "no plugin.json",
        ),
        (vec![("plugin.json", m("nowasm"))], "", "", "installed built"),
        (
            vec![
                ("plugin.json", m("lfs")),
                (
                    "plugin.wasm",
                    b"version https://git-lfs.github.com/spec/v1\n".to_vec(),
                ),
            ],
            "",
            "",
            "Git LFS",
        ),
        (
            vec![("plugin.json", b"{ nope".to_vec()), ("plugin.wasm", ok.clone())],
            "",
            "",
            "invalid",
        ),
        (
            vec![
                (
                    "plugin.json",
                    br#"{"id":"future","name":"f","version":"1","api_version":"9.0"}"#.to_vec(),
                ),
                ("plugin.wasm", ok.clone()),
            ],
            "",
            "",
            "incompatible",
        ),
        (
            vec![
                ("plugin.json", m("junk")),
                ("plugin.wasm", b"\0asm junk".to_vec()),
            ],
            "",
            "",
            "cannot run in rune",
        ),
        (
            vec![
                ("a/plugin.json", m("a")),
                ("a/plugin.wasm", ok.clone()),
                ("b/plugin.json", m("b")),
                ("b/plugin.wasm", ok.clone()),
            ],
            "",
            "",
            "several plugins; give the folder: a, b",
        ),
        (
            vec![("plugin.json", m("x")), ("plugin.wasm", ok.clone())],
            "",
            "nowhere",
            "no folder nowhere",
        ),
        (
            vec![("plugin.json", m("x")), ("plugin.wasm", ok.clone())],
            "no-such-branch",
            "",
            "no branch, tag or version no-such-branch",
        ),
    ];
    for (files, git_ref, subdir, why) in cases {
        let files: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (*p, b.as_slice())).collect();
        let e = try_install(&files, git_ref, subdir).unwrap_err();
        assert!(e.contains(why), "{why:?}: {e}");
    }
    // A module importing more than rune provides is refused before installing.
    let greedy = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))))"#,
    )
    .unwrap();
    let e = try_install(
        &[
            ("plugin.json", &manifest("greedy", "1")),
            ("plugin.wasm", &greedy),
        ],
        "",
        "",
    )
    .unwrap_err();
    assert!(e.contains("wasi_snapshot_preview1.fd_write"), "{e}");

    // The folder of a plugin in a repository of several, and a branch.
    let repo = repo_with(
        &work.0.join("multi"),
        &[
            ("plugins/a/plugin.json", &manifest("a", "1")),
            ("plugins/a/plugin.wasm", &ok),
            ("plugins/b/plugin.json", &manifest("b", "1")),
            ("plugins/b/plugin.wasm", &ok),
        ],
    );
    git_in(&repo, &["checkout", "-q", "-b", "next"]);
    commit(&repo, &[("plugins/b/plugin.json", &manifest("b", "2"))], "b2");
    git_in(&repo, &["checkout", "-q", "main"]);
    let o = install::install(&dir.0, &request(&repo, "next", "plugins/b"), lim, &|_| {}).unwrap();
    assert_eq!((o.id.as_str(), o.version.as_str()), ("b", "2"));
    let src = install::Source::read(&dir.0.join("b")).unwrap();
    assert_eq!(src.track, Track::Branch("next".into()));
    assert_eq!(src.subdir, "plugins/b");
    // Nothing half-installed is left behind by the failures.
    let left: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut left = left;
    left.sort();
    assert_eq!(left, [".staging", "b"]);
    assert_eq!(std::fs::read_dir(dir.0.join(".staging")).unwrap().count(), 0);
}

#[test]
fn modules_are_checked_without_running_them() {
    let lim = Limits::default();
    assert!(Sandbox::check(&says("x"), lim).is_ok());
    // A start function that would loop forever is never run by a check.
    let looping = wat::parse_str(
        r#"(module (memory (export "memory") 1)
            (func $s (loop (br 0))) (start $s)
            (func (export "rune_api_version") (result i32) (i32.const 65536))
            (func (export "rune_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "rune_call") (param i32 i32) (result i64) (i64.const 0)))"#,
    )
    .unwrap();
    assert!(Sandbox::check(&looping, lim).is_ok());
    let wrong = wat::parse_str(
        r#"(module (memory (export "memory") 1)
            (func (export "rune_api_version") (result i32) (i32.const 65536))
            (func (export "rune_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "rune_call") (param i32) (result i32) (i32.const 0)))"#,
    )
    .unwrap();
    assert!(Sandbox::check(&wrong, lim).unwrap_err().contains("rune_call"));
    let no_memory = wat::parse_str(r#"(module)"#).unwrap();
    assert!(Sandbox::check(&no_memory, lim).unwrap_err().contains("memory"));
}

// -- versions and pinning ------------------------------------------------------------

fn tag(repo: &Path, name: &str) {
    git_in(repo, &["tag", "-a", "-m", name, name]);
}

/// A repository with releases v1.0.0, v1.1.0 (annotated tags), v2.0.0-beta
/// and v2.0.0, each one commit of plugin `rel` saying its version, and a
/// commit after the last tag on main.
pub(crate) fn released(work: &Path) -> PathBuf {
    let release = |v: &str| -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("plugin.json", manifest("rel", v)),
            ("plugin.wasm", says(&format!("v{v}"))),
        ]
    };
    let slices = |f: &[(&'static str, Vec<u8>)]| -> Vec<(&'static str, Vec<u8>)> { f.to_vec() };
    let first = slices(&release("1.0.0"));
    let as_refs =
        |f: &Vec<(&'static str, Vec<u8>)>| f.iter().map(|(p, b)| (*p, b.clone())).collect::<Vec<_>>();
    let files = as_refs(&first);
    let borrowed: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (*p, b.as_slice())).collect();
    let repo = repo_with(work, &borrowed);
    tag(&repo, "v1.0.0");
    for v in ["1.1.0", "2.0.0-beta", "2.0.0", "2.0.1-dev"] {
        let f = release(v);
        let borrowed: Vec<(&str, &[u8])> = f.iter().map(|(p, b)| (*p, b.as_slice())).collect();
        commit(&repo, &borrowed, v);
        if v != "2.0.1-dev" {
            tag(&repo, &format!("v{v}"));
        }
    }
    repo
}

pub(crate) fn installed_version(dir: &Path, id: &str) -> String {
    let text = std::fs::read_to_string(dir.join(id).join("plugin.json")).unwrap();
    Manifest::parse(&text).unwrap().version
}

#[test]
fn versions_can_be_followed_pinned_and_switched() {
    if !have_git() {
        return;
    }
    let work = TempDir::new("ver-src");
    let dir = TempDir::new("ver-dst");
    let repo = released(&work.0);
    let lim = Limits::default();
    let up = |to: Option<Track>| install::update(&dir.0, "rel", to, lim, &|_| {});

    // "latest": the newest release, not the pre-release or the commit after it.
    let o = install::install(&dir.0, &request(&repo, "latest", ""), lim, &|_| {}).unwrap();
    assert_eq!((o.version.as_str(), o.tag.as_str()), ("2.0.0", "v2.0.0"));
    assert_eq!(o.track, Track::Releases("*".into()));
    assert_eq!(o.available, None);
    let src = install::Source::read(&dir.0.join("rel")).unwrap();
    assert_eq!(src.version_label(), format!("v2.0.0 ({})", &src.commit[..7]));
    assert!(up(None).unwrap().up_to_date, "nothing newer");

    // An older version, chosen: pinned there; checks report the newer one
    // but install nothing (and look at the remote without downloading).
    let o = up(Some(Track::Pinned("v1.0.0".into()))).unwrap();
    assert!(!o.up_to_date);
    assert_eq!(o.previous.as_ref().unwrap().0, "2.0.0");
    assert_eq!(
        (o.version.as_str(), o.available.as_deref()),
        ("1.0.0", Some("v2.0.0"))
    );
    assert_eq!(installed_version(&dir.0, "rel"), "1.0.0");
    let o = up(None).unwrap();
    assert!(o.up_to_date && o.available.as_deref() == Some("v2.0.0"));
    assert_eq!(installed_version(&dir.0, "rel"), "1.0.0");

    // Compatible releases only: ^1 moves to 1.1.0, never to 2.x.
    let o = up(Some(Track::Releases("^1".into()))).unwrap();
    assert_eq!(
        (o.version.as_str(), o.available.as_deref()),
        ("1.1.0", Some("v2.0.0"))
    );
    commit(
        &repo,
        &[
            ("plugin.json", &manifest("rel", "1.2.0")),
            ("plugin.wasm", &says("v1.2.0")),
        ],
        "1.2",
    );
    tag(&repo, "v1.2.0");
    let o = up(None).unwrap();
    assert_eq!(o.version, "1.2.0", "a new compatible release is taken");

    // Pinning the installed version needs no network and keeps it there.
    assert_eq!(
        install::pin(&dir.0, "rel").unwrap(),
        Track::Pinned("v1.2.0".into())
    );
    let src = install::Source::read(&dir.0.join("rel")).unwrap();
    assert_eq!(src.track, Track::Pinned("v1.2.0".into()));
    assert!(up(None).unwrap().up_to_date);

    // Following the branch: its newest commit (no tag there).
    let o = up(Some(Track::default())).unwrap();
    assert_eq!(
        (o.version.as_str(), o.tag.as_str()),
        ("1.2.0", "v1.2.0"),
        "main's tip is the 1.2 commit"
    );
    commit(&repo, &[("plugin.json", &manifest("rel", "1.3.0-dev"))], "dev");
    let o = up(None).unwrap();
    assert_eq!((o.version.as_str(), o.tag.as_str()), ("1.3.0-dev", ""));

    // A commit: one no branch or tag points at, fetched by its full id…
    let rev = |what: &str| {
        let out = std::process::Command::new("git")
            .args(["rev-parse", what])
            .current_dir(&repo)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let lone = rev("main~2");
    let o = up(Some(Track::Pinned(lone.clone()))).unwrap();
    assert_eq!(
        (o.version.as_str(), o.commit.as_str(), o.tag.as_str()),
        ("2.0.1-dev", lone.as_str(), "")
    );
    // …and one a tag points at (fetched by that tag, which is shown).
    let pinned = rev("v1.1.0^{commit}");
    let o = up(Some(Track::Pinned(pinned.clone()))).unwrap();
    assert_eq!(
        (o.version.as_str(), o.commit.as_str()),
        ("1.1.0", pinned.as_str())
    );
    assert_eq!(o.tag, "v1.1.0", "its tag is found for display");
    let e = up(Some(Track::Pinned(lone[..10].to_owned()))).unwrap_err();
    assert!(e.contains("full 40-character"), "{e}");

    // A range with no match, an unknown tag: refused, nothing changed.
    assert!(
        up(Some(Track::Releases("^7".into())))
            .unwrap_err()
            .contains("no release matches ^7")
    );
    assert!(
        up(Some(Track::Pinned("v9.9.9".into())))
            .unwrap_err()
            .contains("no tag v9.9.9")
    );
    assert_eq!(installed_version(&dir.0, "rel"), "1.1.0");

    // The version list, newest first.
    let v = install::versions(&dir.0, "rel").unwrap();
    let tags: Vec<&str> = v.releases.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(tags, ["v2.0.0", "v2.0.0-beta", "v1.2.0", "v1.1.0", "v1.0.0"]);
    assert_eq!(v.default_branch.as_deref(), Some("main"));

    // Installing by version number names its tag, and pins it.
    let other = TempDir::new("ver-dst2");
    let o = install::install(&other.0, &request(&repo, "1.1", ""), lim, &|_| {}).unwrap();
    assert_eq!(
        (o.version.as_str(), o.track),
        ("1.1.0", Track::Pinned("v1.1.0".into()))
    );
    let e = install::install(&other.0, &request(&repo, "7.0", ""), lim, &|_| {}).unwrap_err();
    assert!(e.contains("no branch, tag or version 7.0"), "{e}");
}

#[test]
fn pinning_or_switching_at_the_same_commit_changes_nothing_that_runs() {
    if !have_git() {
        return;
    }
    let work = TempDir::new("pin-src");
    let dir = TempDir::new("pin-dst");
    let repo = released(&work.0);
    let lim = Limits::default();
    install::install(&dir.0, &request(&repo, "v2.0.0", ""), lim, &|_| {}).unwrap();
    let store = Store::open_in_memory().unwrap();
    let mut p = Plugins::new(store.clone(), Some(dir.0.clone()), false, RESERVED);
    refresh(&mut p, "rel");
    enable(&mut p, "rel");
    // Following releases from the same commit: only the source record changes.
    let o = install::update(&dir.0, "rel", Some(Track::Releases("*".into())), lim, &|_| {}).unwrap();
    assert!(o.up_to_date);
    let (s, jobs) = refresh(&mut p, "rel");
    assert!(s.is_empty() && jobs.is_empty(), "no reload, no message: {s:?}");
    assert_eq!(
        p.get("rel").unwrap().source.as_ref().unwrap().track,
        Track::Releases("*".into())
    );
    assert_eq!(state(&p, "rel"), State::Active);
    install::pin(&dir.0, "rel").unwrap();
    assert!(refresh(&mut p, "rel").0.is_empty());
    assert!(p.get("rel").unwrap().source.as_ref().unwrap().track.is_pinned());
}

#[test]
fn records_from_the_first_version_still_read() {
    let dir = TempDir::new("legacy");
    std::fs::write(
        dir.0.join(install::SOURCE_FILE),
        r#"{"url":"https://h/r","git_ref":"main","subdir":"","commit":"abc1234","installed":1}"#,
    )
    .unwrap();
    let s = install::Source::read(&dir.0).unwrap();
    assert_eq!(s.track, Track::Branch("main".into()));
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        !json.contains("git_ref") && json.contains(r#""track":{"branch":"main"}"#),
        "{json}"
    );
}

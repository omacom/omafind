//! `dig-app`: the Dig search window. A GPUI Shell host that runs the QuickJS UI in `ui/` and lends it one host
//! module, `dig` (search · open · reveal · copy · status · hide). Nothing else reaches the scripts: no filesystem, no
//! processes.
//!
//! Phase 0 spike: the window, the UI, and the host-module round trip. `search` answers with canned results; the
//! daemon socket comes in Phase 4/5.
//!
//!   dig-app [--dev] [--ui DIR]      --dev: hot-reload ui/ on save and write ui/gpui-kit.d.ts

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_shell::{AppAssets, Capabilities, HostError, HostModule, HostObject, HostValue, ShellRuntime};

/// The TypeScript face of the `dig` module. `export_module` checks it against what is registered.
const DIG_TYPES: &str = r#"
/** One search hit: a file, and where in it the match is. */
export interface Result {
  id: number;
  path: string;
  name: string;
  kind: "image" | "screenshot" | "pdf" | "doc" | "audio" | "video" | "code";
  /** Page, seconds, line or character offset; null for a whole-file match (an image). */
  locator: number | null;
  /** The locator for people: "p. 12", "03:41", "L88", or "". */
  locatorLabel: string;
  snippet: string;
}
export interface Answer { query: string; results: Result[]; tookMs: number }
export interface Status { state: string; indexed: number; pending: number }
export function search(query: string, options?: { limit?: number; kinds?: string[] }): Promise<Answer>;
export function status(): Status;
/** Opens a result at its locator (mpv/evince/imv/editor/xdg-open). */
export function open(id: number): boolean;
export function hide(): void;
"#;

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Result<T, HostError> {
    gpui_shell::with_current_app(f).ok_or_else(|| HostError::new("dig: no live App for this call"))
}

/// Canned results for the spike; Phase 4 replaces this with the daemon's answer.
fn canned(query: &str, limit: usize) -> Vec<HostValue> {
    let rows: [(&str, &str, &str, Option<f64>, &str, &str); 6] = [
        ("~/Pictures/2024/beach-day.jpg", "beach-day.jpg", "image", None, "", "a dog running on the beach"),
        ("~/Documents/lease.pdf", "lease.pdf", "pdf", Some(12.0), "p. 12", "Tenants may keep up to two pets with the landlord's written consent"),
        ("~/Music/memos/standup.m4a", "standup.m4a", "audio", Some(221.0), "03:41", "…I haven't been sleeping well, I keep waking up at three…"),
        ("~/Videos/safari.mp4", "safari.mp4", "video", Some(74.0), "01:14", "zebra crossing the road"),
        ("~/Documents/notes/sleep.md", "sleep.md", "doc", Some(0.0), "", "Keep the bedroom cool and dark; no screens before bed."),
        ("~/dev/dig/crates/dig-core/src/lib.rs", "lib.rs", "code", Some(88.0), "L88", "pub fn query(text: &str) -> String"),
    ];
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut hits: Vec<(usize, HostValue)> = rows
        .iter()
        .enumerate()
        .map(|(id, (path, name, kind, locator, label, snippet))| {
            let haystack = format!("{path} {snippet}").to_lowercase();
            let score = words.iter().filter(|w| haystack.contains(w.as_str())).count();
            let value = HostObject::new()
                .field("id", id as f64)
                .field("path", *path)
                .field("name", *name)
                .field("kind", *kind)
                .field("locator", locator.map(HostValue::Number).unwrap_or(HostValue::Null))
                .field("locatorLabel", *label)
                .field("snippet", *snippet);
            (score, HostValue::from(value))
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().take(limit).map(|(_, v)| v).collect()
}

fn dig_module() -> HostModule {
    HostModule::new("dig")
        .declarations(DIG_TYPES)
        .function("status", |_| {
            Ok(HostObject::new().field("state", "spike: no daemon").field("indexed", 0.0).field("pending", 0.0).into())
        })
        .async_function("search", |arguments| {
            // Synchronous half, on the main thread: check the arguments, copy what the work needs out.
            let query = arguments.string(0)?.to_owned();
            let limit = arguments
                .get(1)
                .and_then(|options| options.get("limit"))
                .and_then(|v| match v {
                    HostValue::Number(n) => Some(*n as usize),
                    _ => None,
                })
                .unwrap_or(20);
            let executor = with_app(|cx| cx.background_executor().clone())?;
            // Asynchronous half, on the background executor: what the daemon round trip will be.
            Ok(async move {
                let started = Instant::now();
                executor.timer(Duration::from_millis(15)).await;
                let results = canned(&query, limit);
                Ok(HostObject::new()
                    .field("query", query)
                    .field("results", HostValue::Array(results))
                    .field("tookMs", started.elapsed().as_secs_f64() * 1000.0)
                    .into())
            })
        })
        .function("open", |arguments| {
            let id = arguments.integer(0)?;
            tracing::info!("open result {id} (spike: nothing launched)");
            eprintln!("dig: open result {id}");
            Ok(HostValue::from(true))
        })
        .function("hide", |_| {
            // Phase 5 hides the resident window; the spike just quits.
            eprintln!("dig: hide");
            with_app(|cx| cx.quit())?;
            Ok(HostValue::Null)
        })
}

/// Where the UI scripts are: `--ui DIR`, else `/usr/share/dig/ui` when installed, else the repo's `ui/`.
fn ui_root(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    let installed = PathBuf::from("/usr/share/dig/ui");
    if installed.join("main.js").exists() {
        return installed;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui").canonicalize().unwrap_or_else(|_| PathBuf::from("ui"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dev = cfg!(debug_assertions);
    let mut ui = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dev" => dev = true,
            "--release-ui" => dev = false,
            "--ui" => ui = args.next().map(PathBuf::from),
            other => {
                eprintln!("dig-app: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let ui = ui_root(ui);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("dig_app=info,gpui_shell=info,gpui_kit::shell::script=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let components = match gpui_shell::components::components() {
        Ok(components) => components,
        Err(error) => {
            eprintln!("dig-app: {error:#}");
            std::process::exit(1);
        }
    };
    if dev {
        // Editor completion against exactly the runtime that runs the scripts.
        match gpui_shell::write_type_declarations_with_components(&ui, &components) {
            Ok(written) => tracing::info!("wrote {}", written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Err(error) => tracing::warn!("couldn't write gpui-kit.d.ts: {error}"),
        }
    }

    gpui_kit::application().with_assets(AppAssets::new(ui.clone())).run(move |cx| {
        gpui_shell::init_with_components(cx, &components);
        if dev {
            gpui_shell::set_development_mode(true);
        }
        // The scripts may read their own directory (icons) and nothing else.
        gpui_shell::set_capabilities(Capabilities::new().read_roots([ui.clone()]));
        gpui_shell::export_module(dig_module()).expect("the `dig` module matches its declarations");
        gpui_shell::on_exit_request(|_, _, cx| cx.quit());

        let runtime = match ShellRuntime::new_with_components(cx, components) {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("dig-app: failed to start the script runtime: {error}");
                std::process::exit(1);
            }
        };

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(880.), px(600.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("Dig".into()), ..Default::default() }),
            app_id: Some("dig".into()),
            ..Default::default()
        };
        let app_root = ui.clone();
        let window = cx
            .open_window(options, move |window, cx| {
                let root = runtime.load(&app_root, window, cx);
                if dev {
                    match runtime.watch(&root, window, cx) {
                        Ok(watch) => watch.forget(),
                        Err(error) => tracing::warn!("not watching {}: {error}", app_root.display()),
                    }
                }
                root
            })
            .expect("failed to open the Dig window");

        // Come forward: launched from a terminal, a GPUI window opens behind it. Deferred to a task, outside any
        // platform callback (see gpui-shell's host.rs for the Wayland borrow panic this avoids).
        cx.spawn(async move |cx| {
            cx.update(|cx| {
                cx.activate(true);
                let _ = window.update(cx, |_, window, _| window.activate_window());
            });
        })
        .detach();
    });
}

//! Process icons and agent detection (SPEC 4.6, 4.8).
//!
//! Detection order of [`IconTable::icon_for`]:
//! 1. user mappings from `[icons]` (and `[agents.<name>]`, see [`IconTable::new`]),
//! 2. the foreground process: for interpreters (`node`, `python`, `java`, `bun`, `deno`, `ruby`,
//!    plus the `npx` / `bunx` / `uvx` / `uv run` / `pipx run` launchers) the script or package
//!    in their arguments first, then the executable itself,
//! 3. the title (OSC 0/2) by known agent prefixes (see [`crate::titles`]),
//! 4. otherwise the default `terminal` icon (`>_`).
//!
//! Shells (`zsh`, `bash`, `fish`, `nu`, …) show `terminal`; a nested shell that differs from the
//! login shell shows the `shell` icon (`$`), so `bash` started from `zsh` is visible at a glance.
//! A known foreground process always wins over the title, so a stale agent title left behind in
//! a shell never shows an agent icon.
//!
//! Every lookup is a hash-map hit; nothing touches the filesystem.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::titles::title_agent_kind;

/// The foreground process of a pane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessInfo {
    /// Executable basename (`node`, `claude`, `python3.12`).
    pub exe: String,
    /// Arguments after the executable (argv[1..]).
    pub args: Vec<String>,
}

impl ProcessInfo {
    pub fn new<S: Into<String>>(exe: impl Into<String>, args: impl IntoIterator<Item = S>) -> Self {
        Self { exe: exe.into(), args: args.into_iter().map(Into::into).collect() }
    }
}

/// An icon to draw for a tab / pane.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Icon {
    /// A bundled icon, one of [`builtin_icon_names`]; its SVG is
    /// `assets/icons/process/<name>.svg` (also [`builtin_icon_svg`]).
    Builtin(&'static str),
    /// A user SVG (from `[icons]` / `[agents.<name>].icon`).
    Custom(PathBuf),
}

impl Icon {
    pub const TERMINAL: Icon = Icon::Builtin("terminal");
}

macro_rules! builtin_icons {
    ($($name:literal),* $(,)?) => {
        const BUILTIN_ICONS: &[&str] = &[$($name),*];

        /// The bundled SVG source of a built-in icon.
        pub fn builtin_icon_svg(name: &str) -> Option<&'static str> {
            match name {
                $($name => Some(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"), "/../../assets/icons/process/", $name, ".svg"
                ))),)*
                _ => None,
            }
        }
    };
}

builtin_icons!(
    // generic
    "terminal",
    "shell",
    "agent",
    // AI coding agents
    "claude",
    "codex",
    "gemini",
    "copilot",
    "aider",
    "cursor",
    "opencode",
    "amp",
    "goose",
    "ollama",
    // languages / runtimes
    "python",
    "java",
    "kotlin",
    "scala",
    "node",
    "bun",
    "deno",
    "ruby",
    "go",
    "rust",
    "swift",
    "php",
    "elixir",
    "haskell",
    "lua",
    "r",
    "julia",
    "dotnet",
    // build tools
    "gradle",
    "maven",
    "make",
    "cmake",
    "bazel",
    // version control
    "git",
    "jujutsu",
    // containers / cloud / infra
    "docker",
    "kubernetes",
    "terraform",
    "aws",
    "gcloud",
    "azure",
    "fly",
    // editors
    "vim",
    "emacs",
    "helix",
    "nano",
    "micro",
    // databases
    "postgres",
    "mysql",
    "sqlite",
    "redis",
    "mongodb",
    // remote
    "ssh",
    // monitoring / tools
    "htop",
    "tmux",
    "zellij",
    "less",
    "curl",
    "ffmpeg",
    "brew",
);

/// Every built-in icon name (`terminal`, `shell`, `agent`, `claude`, `python`, …).
pub fn builtin_icon_names() -> &'static [&'static str] {
    BUILTIN_ICONS
}

fn builtin_name(name: &str) -> Option<&'static str> {
    BUILTIN_ICONS.iter().copied().find(|n| *n == name)
}

/// Built-in agent kinds and their icons (SPEC 4.6 / 4.8).
pub const BUILTIN_AGENT_KINDS: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("gemini", "gemini"),
    ("copilot", "copilot"),
    ("aider", "aider"),
    ("cursor-agent", "cursor"),
    ("opencode", "opencode"),
    ("amp", "amp"),
    ("goose", "goose"),
    ("ollama", "ollama"),
];

/// Executable → built-in agent kind.
const AGENT_EXES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("claude-code", "claude"),
    ("codex", "codex"),
    ("gemini", "gemini"),
    ("gemini-cli", "gemini"),
    ("copilot", "copilot"),
    ("github-copilot", "copilot"),
    ("aider", "aider"),
    ("cursor-agent", "cursor-agent"),
    ("opencode", "opencode"),
    ("amp", "amp"),
    ("goose", "goose"),
    ("ollama", "ollama"),
];

/// Executable → built-in icon. Agent executables come from [`AGENT_EXES`].
const EXE_ICONS: &[(&str, &str)] = &[
    // languages / runtimes / REPLs
    ("python", "python"),
    ("ipython", "python"),
    ("bpython", "python"),
    ("pypy", "python"),
    ("uv", "python"),
    ("uvx", "python"),
    ("poetry", "python"),
    ("pip", "python"),
    ("pipx", "python"),
    ("pdm", "python"),
    ("hatch", "python"),
    ("conda", "python"),
    ("jupyter", "python"),
    ("pytest", "python"),
    ("java", "java"),
    ("jshell", "java"),
    ("javac", "java"),
    ("kotlin", "kotlin"),
    ("kotlinc", "kotlin"),
    ("ki", "kotlin"),
    ("scala", "scala"),
    ("sbt", "scala"),
    ("scala-cli", "scala"),
    ("amm", "scala"),
    ("sbt-launch", "scala"),
    ("node", "node"),
    ("nodejs", "node"),
    ("npm", "node"),
    ("pnpm", "node"),
    ("yarn", "node"),
    ("npx", "node"),
    ("corepack", "node"),
    ("nodemon", "node"),
    ("tsx", "node"),
    ("ts-node", "node"),
    ("bun", "bun"),
    ("bunx", "bun"),
    ("deno", "deno"),
    ("ruby", "ruby"),
    ("irb", "ruby"),
    ("rails", "ruby"),
    ("bundle", "ruby"),
    ("bundler", "ruby"),
    ("rake", "ruby"),
    ("gem", "ruby"),
    ("pry", "ruby"),
    ("go", "go"),
    ("gofmt", "go"),
    ("dlv", "go"),
    ("cargo", "rust"),
    ("rustc", "rust"),
    ("rustup", "rust"),
    ("bacon", "rust"),
    ("swift", "swift"),
    ("swiftc", "swift"),
    ("xcodebuild", "swift"),
    ("xcrun", "swift"),
    ("php", "php"),
    ("composer", "php"),
    ("iex", "elixir"),
    ("mix", "elixir"),
    ("elixir", "elixir"),
    ("erl", "elixir"),
    ("ghci", "haskell"),
    ("ghc", "haskell"),
    ("stack", "haskell"),
    ("cabal", "haskell"),
    ("lua", "lua"),
    ("luajit", "lua"),
    ("r", "r"),
    ("rscript", "r"),
    ("julia", "julia"),
    ("dotnet", "dotnet"),
    // build tools
    ("gradle", "gradle"),
    ("gradlew", "gradle"),
    ("mvn", "maven"),
    ("mvnw", "maven"),
    ("make", "make"),
    ("gmake", "make"),
    ("cmake", "cmake"),
    ("ctest", "cmake"),
    ("ninja", "cmake"),
    ("bazel", "bazel"),
    ("bazelisk", "bazel"),
    // version control
    ("git", "git"),
    ("lazygit", "git"),
    ("tig", "git"),
    ("gh", "git"),
    ("gitui", "git"),
    ("jj", "jujutsu"),
    // containers / cloud / infra
    ("docker", "docker"),
    ("docker-compose", "docker"),
    ("lazydocker", "docker"),
    ("podman", "docker"),
    ("colima", "docker"),
    ("kubectl", "kubernetes"),
    ("k9s", "kubernetes"),
    ("helm", "kubernetes"),
    ("kubectx", "kubernetes"),
    ("kubens", "kubernetes"),
    ("minikube", "kubernetes"),
    ("kind", "kubernetes"),
    ("stern", "kubernetes"),
    ("terraform", "terraform"),
    ("tofu", "terraform"),
    ("terragrunt", "terraform"),
    ("aws", "aws"),
    ("gcloud", "gcloud"),
    ("gsutil", "gcloud"),
    ("az", "azure"),
    ("fly", "fly"),
    ("flyctl", "fly"),
    // editors
    ("vim", "vim"),
    ("nvim", "vim"),
    ("vi", "vim"),
    ("view", "vim"),
    ("vimdiff", "vim"),
    ("emacs", "emacs"),
    ("emacsclient", "emacs"),
    ("hx", "helix"),
    ("helix", "helix"),
    ("nano", "nano"),
    ("micro", "micro"),
    // databases
    ("psql", "postgres"),
    ("pgcli", "postgres"),
    ("postgres", "postgres"),
    ("pg_dump", "postgres"),
    ("mysql", "mysql"),
    ("mycli", "mysql"),
    ("mariadb", "mysql"),
    ("sqlite3", "sqlite"),
    ("sqlite", "sqlite"),
    ("litecli", "sqlite"),
    ("redis-cli", "redis"),
    ("redis-server", "redis"),
    ("valkey-cli", "redis"),
    ("mongosh", "mongodb"),
    ("mongo", "mongodb"),
    ("mongod", "mongodb"),
    // remote / network
    ("ssh", "ssh"),
    ("mosh", "ssh"),
    ("mosh-client", "ssh"),
    ("et", "ssh"),
    ("autossh", "ssh"),
    ("sftp", "ssh"),
    // monitoring / tools
    ("htop", "htop"),
    ("btop", "htop"),
    ("top", "htop"),
    ("btm", "htop"),
    ("atop", "htop"),
    ("glances", "htop"),
    ("tmux", "tmux"),
    ("zellij", "zellij"),
    ("less", "less"),
    ("man", "less"),
    ("more", "less"),
    ("most", "less"),
    ("curl", "curl"),
    ("wget", "curl"),
    ("http", "curl"),
    ("xh", "curl"),
    ("ffmpeg", "ffmpeg"),
    ("ffprobe", "ffmpeg"),
    ("brew", "brew"),
];

/// Shell executables: shown as `terminal` (or `shell` when nested and different from the login
/// shell).
const SHELLS: &[&str] =
    &["zsh", "bash", "fish", "nu", "sh", "dash", "ksh", "tcsh", "csh", "xonsh", "elvish", "pwsh"];

/// npm / PyPI packages of agents, matched as substrings of interpreter arguments
/// (`node …/node_modules/@openai/codex/bin/codex.js` → codex).
const AGENT_PACKAGES: &[(&str, &str)] = &[
    ("@anthropic-ai/claude-code", "claude"),
    ("@openai/codex", "codex"),
    ("@google/gemini-cli", "gemini"),
    ("@github/copilot", "copilot"),
    ("opencode-ai", "opencode"),
    ("@sourcegraph/amp", "amp"),
    ("aider-chat", "aider"),
];

struct Builtins {
    icons: HashMap<&'static str, &'static str>,
    agents: HashMap<&'static str, &'static str>,
}

fn builtins() -> &'static Builtins {
    static B: OnceLock<Builtins> = OnceLock::new();
    B.get_or_init(|| {
        let mut icons: HashMap<&'static str, &'static str> = EXE_ICONS.iter().copied().collect();
        let agent_icon: HashMap<_, _> = BUILTIN_AGENT_KINDS.iter().copied().collect();
        for (exe, kind) in AGENT_EXES {
            icons.insert(exe, agent_icon[kind]);
        }
        Builtins { icons, agents: AGENT_EXES.iter().copied().collect() }
    })
}

/// Normalized lookup key of an executable: basename, lowercase, without a login-shell `-`.
fn normalize_exe(exe: &str) -> String {
    let base = exe.rsplit('/').next().unwrap_or(exe);
    base.trim_start_matches('-').to_ascii_lowercase()
}

/// `python3.12` → `python`, `node20` → `node`, `sbt-launch-1.9` → `sbt-launch`.
fn strip_version(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').trim_end_matches(['-', '_', '@'])
}

/// Looks `name` up exactly, then without a version suffix.
fn lookup<'m, V>(
    map: &'m HashMap<impl std::borrow::Borrow<str> + std::hash::Hash + Eq, V>,
    name: &str,
) -> Option<&'m V> {
    map.get(name).or_else(|| {
        let stripped = strip_version(name);
        (stripped != name && !stripped.is_empty()).then(|| map.get(stripped)).flatten()
    })
}

/// Which arguments of an interpreter name the program it runs.
fn interpreter_target(exe: &str, args: &[String]) -> Option<String> {
    let first_non_flag = |args: &[String], skip_values: &[&str]| -> Option<String> {
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--" {
                return it.next().cloned();
            }
            if a.starts_with('-') {
                if skip_values.contains(&a.as_str()) {
                    it.next();
                }
                continue;
            }
            return Some(a.clone());
        }
        None
    };
    let after_sub = |args: &[String], subs: &[&str]| -> Vec<String> {
        match args.first() {
            Some(a) if subs.contains(&a.as_str()) => args[1..].to_vec(),
            _ => args.to_vec(),
        }
    };
    let exe = strip_version(exe);
    match exe {
        "node" | "nodejs" => {
            if args.iter().any(|a| matches!(a.as_str(), "-e" | "--eval" | "-p" | "--print")) {
                return None;
            }
            first_non_flag(
                args,
                &["-r", "--require", "--import", "--loader", "--experimental-loader", "-C", "--conditions"],
            )
        }
        "bun" => first_non_flag(&after_sub(args, &["run", "x"]), &[]),
        "deno" => first_non_flag(&after_sub(args, &["run"]), &[]),
        "npx" | "bunx" | "uvx" | "pnpx" => first_non_flag(args, &["-p", "--package", "--from"]),
        "pnpm" | "yarn" => match args.first().map(String::as_str) {
            Some("dlx") => first_non_flag(&args[1..], &[]),
            _ => None,
        },
        "uv" => match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
            (Some("run"), _) => first_non_flag(&args[1..], &["--with", "--python", "-p", "--from"]),
            (Some("tool"), Some("run")) => first_non_flag(&args[2..], &["--with", "--from"]),
            _ => None,
        },
        "pipx" => match args.first().map(String::as_str) {
            Some("run") => first_non_flag(&args[1..], &["--spec"]),
            _ => None,
        },
        "python" | "pypy" => {
            let mut it = args.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-m" => {
                        return it.next().map(|m| m.split('.').next().unwrap_or(m).to_string());
                    }
                    "-c" => return None,
                    "-X" | "-W" | "-Q" => {
                        it.next();
                    }
                    s if s.starts_with('-') => {}
                    s => return Some(s.to_string()),
                }
            }
            None
        }
        "ruby" => {
            let mut it = args.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-S" => return it.next().cloned(),
                    "-e" => return None,
                    "-I" | "-r" => {
                        it.next();
                    }
                    s if s.starts_with('-') => {}
                    s => return Some(s.to_string()),
                }
            }
            None
        }
        "java" => {
            let pos = args.iter().position(|a| a == "-jar")?;
            args.get(pos + 1).cloned()
        }
        "gh" => (args.first().map(String::as_str) == Some("copilot")).then(|| "copilot".into()),
        _ => None,
    }
}

/// The program name an interpreter runs: an agent package, else the script's basename without
/// its extension (`/usr/local/bin/claude` → `claude`, `app.jar` → `app`).
fn script_name(exe: &str, args: &[String]) -> Option<String> {
    let target = interpreter_target(exe, args)?;
    let lower = target.to_ascii_lowercase();
    if let Some((_, kind)) = AGENT_PACKAGES.iter().find(|(pkg, _)| lower.contains(pkg)) {
        return Some((*kind).to_string());
    }
    // A package spec (`cowsay@1.2`, `@scope/pkg@latest`): drop the version.
    let spec = match lower.rfind('@') {
        Some(i) if i > 0 => &lower[..i],
        _ => &lower[..],
    };
    let base = spec.trim_end_matches('/').rsplit('/').next()?;
    let base = ["js", "mjs", "cjs", "ts", "mts", "py", "rb", "jar"]
        .iter()
        .find_map(|ext| base.strip_suffix(&format!(".{ext}")))
        .unwrap_or(base);
    (!base.is_empty()).then(|| base.to_string())
}

/// Launchers run packages / archives named after the tool (`npx cowsay`, `java -jar sbt-launch.jar`).
fn is_launcher(exe: &str) -> bool {
    matches!(
        strip_version(exe),
        "npx" | "bunx" | "uvx" | "pnpx" | "pipx" | "uv" | "pnpm" | "yarn" | "java" | "gh"
    )
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name) || SHELLS.contains(&strip_version(name))
}

/// Process icon and agent lookup table: the built-in tables merged with the user's `[icons]` and
/// `[agents.<name>]` configuration. Build it once per config load.
#[derive(Debug, Clone, Default)]
pub struct IconTable {
    /// executable / agent kind (lowercase) → user icon
    user: HashMap<String, Icon>,
    /// configured executable (lowercase) → agent kind
    agent_exes: HashMap<String, String>,
}

impl IconTable {
    /// `user_icons`: `[icons]` (executable → built-in icon name or SVG path). An `[agents.<name>]
    /// icon` can be passed here keyed by the agent name. Paths may start with `~`; a bare
    /// `file.svg` is taken from `~/.nox/icons/`. Unknown built-in names are ignored (logged).
    ///
    /// `agent_matches`: `[agents.<name>]` as (kind, executables) — the key itself plus `match`.
    pub fn new(user_icons: &BTreeMap<String, String>, agent_matches: &[(String, Vec<String>)]) -> Self {
        let mut user = HashMap::new();
        for (exe, value) in user_icons {
            match parse_user_icon(value) {
                Some(icon) => {
                    user.insert(normalize_exe(exe), icon);
                }
                None => log::warn!("[icons] {exe:?}: unknown icon {value:?}"),
            }
        }
        let mut agent_exes = HashMap::new();
        for (kind, exes) in agent_matches {
            agent_exes.insert(normalize_exe(kind), kind.clone());
            for exe in exes {
                agent_exes.insert(normalize_exe(exe), kind.clone());
            }
        }
        Self { user, agent_exes }
    }

    /// Candidate program names of a process, most specific first: the interpreted script /
    /// package, then the executable.
    fn candidates(proc: &ProcessInfo) -> (Option<String>, String) {
        let exe = normalize_exe(&proc.exe);
        (script_name(&exe, &proc.args), exe)
    }

    /// The icon for a pane (see the module docs for the order).
    pub fn icon_for(&self, proc: Option<&ProcessInfo>, title: Option<&str>, login_shell: &str) -> Icon {
        if let Some(proc) = proc {
            let (script, exe) = Self::candidates(proc);
            if let Some(script) = &script
                && let Some(icon) = self.icon_for_name(script, is_launcher(&exe))
            {
                return icon;
            }
            if let Some(icon) = self.user.get(&exe).or_else(|| lookup(&self.user, &exe)) {
                return icon.clone();
            }
            if is_shell(&exe) {
                let login = normalize_exe(login_shell);
                return if login.is_empty() || strip_version(&login) == strip_version(&exe) {
                    Icon::TERMINAL
                } else {
                    Icon::Builtin("shell")
                };
            }
            if let Some(icon) = self.icon_for_name(&exe, true) {
                return icon;
            }
        }
        if let Some(kind) = title.and_then(title_agent_kind) {
            return self.icon_for_kind(kind);
        }
        Icon::TERMINAL
    }

    /// `tools`: also match built-in non-agent tools. Off for scripts run by a plain interpreter,
    /// so `python -m http.server` stays Python instead of matching the `http` client.
    fn icon_for_name(&self, name: &str, tools: bool) -> Option<Icon> {
        if let Some(icon) = lookup(&self.user, name) {
            return Some(icon.clone());
        }
        if let Some(kind) = lookup(&self.agent_exes, name) {
            return Some(self.icon_for_kind(kind));
        }
        if let Some(kind) = lookup(&builtins().agents, name) {
            return Some(self.icon_for_kind(kind));
        }
        if !tools {
            return None;
        }
        lookup(&builtins().icons, name).map(|n| Icon::Builtin(n))
    }

    /// The icon of an agent kind: a user icon for it, its built-in icon, else the generic
    /// `agent` icon.
    pub fn icon_for_kind(&self, kind: &str) -> Icon {
        if let Some(icon) = self.user.get(&kind.to_ascii_lowercase()) {
            return icon.clone();
        }
        if let Some((_, icon)) = BUILTIN_AGENT_KINDS.iter().find(|(k, _)| *k == kind) {
            return Icon::Builtin(icon);
        }
        match builtin_name(kind) {
            Some(name) => Icon::Builtin(name),
            None => Icon::Builtin("agent"),
        }
    }

    /// The agent kind running in a pane, if any: a configured `[agents.<name>]` executable, a
    /// built-in agent, or — only when the foreground process is unknown or absent — an agent
    /// title prefix.
    pub fn agent_kind(&self, proc: Option<&ProcessInfo>, title: Option<&str>) -> Option<String> {
        if let Some(proc) = proc {
            let (script, exe) = Self::candidates(proc);
            for name in script.iter().chain(std::iter::once(&exe)) {
                if let Some(kind) = lookup(&self.agent_exes, name) {
                    return Some(kind.clone());
                }
                if let Some(kind) = lookup(&builtins().agents, name) {
                    return Some((*kind).to_string());
                }
            }
            let known = script
                .iter()
                .chain(std::iter::once(&exe))
                .any(|n| lookup(&builtins().icons, n).is_some() || lookup(&self.user, n).is_some())
                || is_shell(&exe);
            if known {
                return None;
            }
        }
        title.and_then(title_agent_kind).map(str::to_string)
    }
}

fn parse_user_icon(value: &str) -> Option<Icon> {
    let value = value.trim();
    if let Some(name) = builtin_name(value) {
        return Some(Icon::Builtin(name));
    }
    let home = dirs::home_dir();
    if let Some(rest) = value.strip_prefix("~/") {
        return Some(Icon::Custom(home.map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(value))));
    }
    if value.contains('/') {
        return Some(Icon::Custom(PathBuf::from(value)));
    }
    if value.ends_with(".svg") {
        let dir = home.map(|h| h.join(".nox/icons")).unwrap_or_default();
        return Some(Icon::Custom(dir.join(value)));
    }
    None
}

/// Brand color of a built-in icon as `0xRRGGBB`, for the optional tint. The app blends it toward
/// the text color (low saturation) so it reads in light and dark mode. `None` for monochrome
/// brands and generic icons.
pub fn brand_tint(name: &str) -> Option<u32> {
    Some(match name {
        "claude" => 0xD97757,
        "codex" => 0x10A37F,
        "gemini" => 0x4285F4,
        "copilot" => 0x8957E5,
        "aider" => 0x14B014,
        "python" => 0x3776AB,
        "java" => 0xE76F00,
        "kotlin" => 0x7F52FF,
        "scala" => 0xDC322F,
        "node" => 0x5FA04E,
        "bun" => 0xF472B6,
        "ruby" => 0xCC342D,
        "go" => 0x00ADD8,
        "rust" => 0xCE422B,
        "swift" => 0xF05138,
        "php" => 0x777BB4,
        "elixir" => 0x6E4A7E,
        "haskell" => 0x5E5086,
        "lua" => 0x2C2D72,
        "r" => 0x276DC3,
        "julia" => 0x9558B2,
        "dotnet" => 0x512BD4,
        "gradle" => 0x209BC4,
        "maven" => 0xC71A36,
        "cmake" => 0x064F8C,
        "bazel" => 0x43A047,
        "git" => 0xF05032,
        "docker" => 0x2496ED,
        "kubernetes" => 0x326CE5,
        "terraform" => 0x7B42BC,
        "aws" => 0xFF9900,
        "gcloud" => 0x4285F4,
        "azure" => 0x0078D4,
        "fly" => 0x8B5CF6,
        "vim" => 0x019733,
        "emacs" => 0x7F5AB6,
        "helix" => 0x706CC2,
        "postgres" => 0x4169E1,
        "mysql" => 0x4479A1,
        "sqlite" => 0x0F80CC,
        "redis" => 0xDC382D,
        "mongodb" => 0x47A248,
        "tmux" => 0x1BB91F,
        "zellij" => 0xE8A23B,
        "ffmpeg" => 0x007808,
        "brew" => 0xFBB040,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(exe: &str, args: &[&str]) -> ProcessInfo {
        ProcessInfo::new(exe, args.iter().copied())
    }

    fn table() -> IconTable {
        IconTable::new(&BTreeMap::new(), &[])
    }

    fn icon(t: &IconTable, proc: Option<&ProcessInfo>, title: Option<&str>) -> Icon {
        t.icon_for(proc, title, "/bin/zsh")
    }

    #[test]
    fn every_builtin_has_an_svg_and_style() {
        for name in builtin_icon_names() {
            let svg = builtin_icon_svg(name).unwrap();
            assert!(svg.contains("viewBox=\"0 0 16 16\""), "{name}");
            assert!(svg.contains("stroke-width=\"1.5\""), "{name}");
            assert!(!svg.contains("href"), "{name}");
            assert!(svg.len() < 1024, "{name} is not tiny");
        }
        for (_, icon) in BUILTIN_AGENT_KINDS {
            assert!(builtin_name(icon).is_some());
        }
        for (_, icon) in EXE_ICONS {
            assert!(builtin_name(icon).is_some(), "{icon}");
        }
        for name in builtin_icon_names() {
            if let Some(c) = brand_tint(name) {
                assert!(c <= 0xFFFFFF);
            }
        }
    }

    #[test]
    fn plain_executables() {
        let t = table();
        let cases = [
            ("claude", "claude"),
            ("codex", "codex"),
            ("cursor-agent", "cursor"),
            ("nvim", "vim"),
            ("vim", "vim"),
            ("kubectl", "kubernetes"),
            ("k9s", "kubernetes"),
            ("helm", "kubernetes"),
            ("python3.12", "python"),
            ("python3", "python"),
            ("cargo", "rust"),
            ("lazygit", "git"),
            ("jj", "jujutsu"),
            ("psql", "postgres"),
            ("redis-cli", "redis"),
            ("mosh", "ssh"),
            ("btop", "htop"),
            ("man", "less"),
            ("wget", "curl"),
            ("brew", "brew"),
            ("tofu", "terraform"),
            ("flyctl", "fly"),
            ("xcodebuild", "swift"),
            ("R", "r"),
            ("node20", "node"),
            ("docker-compose", "docker"),
        ];
        for (exe, want) in cases {
            assert_eq!(icon(&t, Some(&p(exe, &[])), None), Icon::Builtin(want), "{exe}");
        }
        assert_eq!(icon(&t, Some(&p("some-unknown-tool", &[])), None), Icon::TERMINAL);
        assert_eq!(icon(&t, None, None), Icon::TERMINAL);
    }

    #[test]
    fn interpreter_arguments() {
        let t = table();
        let cases: &[(&str, &[&str], &str)] = &[
            ("node", &["/usr/local/bin/claude"], "claude"),
            (
                "node",
                &["--no-warnings", "/opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js"],
                "codex",
            ),
            ("node", &["/x/node_modules/@anthropic-ai/claude-code/cli.js", "--resume", "abc"], "claude"),
            ("node", &["/x/node_modules/@google/gemini-cli/dist/index.js"], "gemini"),
            ("node", &["server.js"], "node"),
            ("node", &["-e", "console.log(1)"], "node"),
            ("node", &[], "node"),
            ("java", &["-jar", "app.jar"], "java"),
            ("java", &["-Xmx1g", "-jar", "/x/sbt-launch-1.9.jar"], "scala"),
            ("python3", &["-m", "aider"], "aider"),
            ("python3.11", &["/Users/me/.local/bin/aider", "--model", "x"], "aider"),
            ("python", &["manage.py", "runserver"], "python"),
            ("python", &["-m", "http.server"], "python"),
            ("bun", &["run", "/x/node_modules/opencode-ai/bin/opencode"], "opencode"),
            ("npx", &["@anthropic-ai/claude-code"], "claude"),
            ("npx", &["-y", "@openai/codex@latest"], "codex"),
            ("uvx", &["--from", "aider-chat", "aider"], "aider"),
            ("deno", &["run", "-A", "main.ts"], "deno"),
            ("ruby", &["bin/rails", "server"], "ruby"),
            ("gh", &["copilot", "suggest"], "copilot"),
            ("gh", &["pr", "list"], "git"),
        ];
        for (exe, args, want) in cases {
            assert_eq!(icon(&t, Some(&p(exe, args)), None), Icon::Builtin(want), "{exe} {args:?}");
        }
    }

    #[test]
    fn shells_and_titles() {
        let t = table();
        assert_eq!(t.icon_for(Some(&p("zsh", &[])), None, "/bin/zsh"), Icon::TERMINAL);
        assert_eq!(t.icon_for(Some(&p("-zsh", &[])), None, "-zsh"), Icon::TERMINAL);
        assert_eq!(t.icon_for(Some(&p("bash", &[])), None, "/bin/zsh"), Icon::Builtin("shell"));
        // A stale agent title in a shell does not show the agent.
        assert_eq!(t.icon_for(Some(&p("zsh", &[])), Some("✳ Claude Code"), "/bin/zsh"), Icon::TERMINAL);
        // Unknown process (Claude's native binary is named after its version) → title.
        assert_eq!(icon(&t, Some(&p("2.1.3", &[])), Some("✳ Claude Code")), Icon::Builtin("claude"));
        assert_eq!(icon(&t, None, Some("✳ Fix the parser")), Icon::Builtin("claude"));
        assert_eq!(icon(&t, None, Some("pw@mbp:~")), Icon::TERMINAL);
        assert_eq!(t.agent_kind(Some(&p("zsh", &[])), Some("✳ Claude Code")), None);
        assert_eq!(t.agent_kind(Some(&p("2.1.3", &[])), Some("✳ Claude Code")).as_deref(), Some("claude"));
    }

    #[test]
    fn user_mappings_and_configured_agents() {
        let mut user = BTreeMap::new();
        user.insert("jbang".to_string(), "java".to_string());
        user.insert("my-agent".to_string(), "claude".to_string());
        user.insert("work-cli".to_string(), "/icons/work.svg".to_string());
        user.insert("nvim".to_string(), "emacs".to_string());
        user.insert("bogus".to_string(), "not-an-icon".to_string());
        let agents = vec![
            ("droid".to_string(), vec!["droid-dev".to_string()]),
            ("claude".to_string(), vec!["claude-nightly".to_string()]),
        ];
        let t = IconTable::new(&user, &agents);
        assert_eq!(icon(&t, Some(&p("jbang", &[])), None), Icon::Builtin("java"));
        assert_eq!(icon(&t, Some(&p("nvim", &[])), None), Icon::Builtin("emacs"));
        assert_eq!(icon(&t, Some(&p("work-cli", &[])), None), Icon::Custom("/icons/work.svg".into()));
        assert_eq!(icon(&t, Some(&p("bogus", &[])), None), Icon::TERMINAL);
        assert_eq!(icon(&t, Some(&p("droid-dev", &[])), None), Icon::Builtin("agent"));
        assert_eq!(icon(&t, Some(&p("claude-nightly", &[])), None), Icon::Builtin("claude"));
        assert_eq!(t.agent_kind(Some(&p("droid-dev", &[])), None).as_deref(), Some("droid"));
        assert_eq!(t.agent_kind(Some(&p("droid", &[])), None).as_deref(), Some("droid"));
        assert_eq!(t.agent_kind(Some(&p("claude-nightly", &[])), None).as_deref(), Some("claude"));
        // A user icon mapping alone does not make a program an agent.
        assert_eq!(t.agent_kind(Some(&p("my-agent", &[])), None), None);
    }

    #[test]
    fn agent_kinds() {
        let t = table();
        for (exe, kind) in [
            ("claude", "claude"),
            ("codex", "codex"),
            ("gemini", "gemini"),
            ("copilot", "copilot"),
            ("aider", "aider"),
            ("cursor-agent", "cursor-agent"),
            ("opencode", "opencode"),
            ("amp", "amp"),
            ("goose", "goose"),
            ("ollama", "ollama"),
        ] {
            assert_eq!(t.agent_kind(Some(&p(exe, &[])), None).as_deref(), Some(kind));
        }
        assert_eq!(t.agent_kind(Some(&p("node", &["/usr/local/bin/codex"])), None).as_deref(), Some("codex"));
        assert_eq!(t.agent_kind(Some(&p("node", &["server.js"])), None), None);
        assert_eq!(t.agent_kind(Some(&p("vim", &[])), None), None);
        assert_eq!(t.agent_kind(None, None), None);
    }
}

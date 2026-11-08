const root = document.documentElement;
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------------------------
// Page theme: System (default) → Light → Dark, persisted in localStorage.
const toggle = document.querySelector(".theme-toggle");
const MODES = ["system", "light", "dark"];
const LABELS = { system: "System", light: "Light", dark: "Dark" };
const darkQuery = matchMedia("(prefers-color-scheme: dark)");

function currentMode() {
  const t = root.dataset.theme;
  return t === "light" || t === "dark" ? t : "system";
}

function pageIsDark() {
  const mode = currentMode();
  return mode === "dark" || (mode === "system" && darkQuery.matches);
}

function applyMode(mode) {
  if (mode === "system") delete root.dataset.theme;
  else root.dataset.theme = mode;
  try {
    if (mode === "system") localStorage.removeItem("nox-theme");
    else localStorage.setItem("nox-theme", mode);
  } catch {}
  toggle.dataset.mode = mode;
  toggle.title = toggle.ariaLabel = `Theme: ${LABELS[mode]}`;
  applyTermTheme();
}

toggle.addEventListener("click", () => {
  applyMode(MODES[(MODES.indexOf(currentMode()) + 1) % MODES.length]);
});
darkQuery.addEventListener("change", () => applyTermTheme());

// ---------------------------------------------------------------------------------------------
// The window preview uses the built-in `nox` palette, light or dark like the page.
const NOX = {
  dark: {
    bg: "#18191b", "bg-sidebar": "#1c1d20", "bg-elevated": "#2a2b2f", text: "#e6e6e8", "text-muted": "#8a8a8f",
    border: "#2a2a2e", accent: "#22d3ee",
    ansi: { red: "#f07178", green: "#8bd49c", yellow: "#e6c07b", blue: "#6cb6ff", magenta: "#f075c1", cyan: "#5ccfe6" },
  },
  light: {
    bg: "#ffffff", "bg-sidebar": "#f7f7f8", "bg-elevated": "#efeff1", text: "#1d1d1f", "text-muted": "#8a8a8e",
    border: "#e6e6e8", accent: "#0891b2",
    ansi: { red: "#c4262e", green: "#18792f", yellow: "#8a6a00", blue: "#0b63ce", magenta: "#a5307f", cyan: "#0a7a8c" },
  },
};

const term = document.getElementById("term");

function applyTermTheme() {
  const v = pageIsDark() ? NOX.dark : NOX.light;
  const a = v.ansi;
  const vars = {
    "--t-bg": v.bg, "--t-sidebar": v["bg-sidebar"] ?? v.bg, "--t-fg": v.text,
    "--t-muted": v["text-muted"], "--t-border": v.border, "--t-accent": v.accent,
    "--t-red": a.red, "--t-green": a.green, "--t-yellow": a.yellow,
    "--t-blue": a.blue, "--t-magenta": a.magenta, "--t-cyan": a.cyan,
    "--t-elevated": v["bg-elevated"],
  };
  for (const [k, val] of Object.entries(vars)) term.style.setProperty(k, val);
}

applyMode(currentMode());

// ---------------------------------------------------------------------------------------------
// Install command: a click copies it.
document.querySelectorAll(".install-cmd").forEach((button) => {
  const label = button.ariaLabel;
  let timer;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(button.querySelector("code").textContent);
    } catch {
      return;
    }
    button.classList.add("copied");
    button.ariaLabel = "Copied";
    clearTimeout(timer);
    timer = setTimeout(() => {
      button.classList.remove("copied");
      button.ariaLabel = label;
    }, 1500);
  });
});

// ---------------------------------------------------------------------------------------------
// Scroll reveal, staggered within each section.
document.querySelectorAll("section").forEach((section) => {
  section.querySelectorAll(".reveal").forEach((el, i) => {
    el.style.setProperty("--delay", `${Math.min(i, 6) * 70}ms`);
  });
});

const observer = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting) {
        entry.target.classList.add("in");
        entry.target.addEventListener("transitionend", () => entry.target.classList.add("done"), { once: true });
        observer.unobserve(entry.target);
      }
    }
  },
  { threshold: 0.12, rootMargin: "0px 0px -40px 0px" },
);
document.querySelectorAll(".reveal").forEach((el) => observer.observe(el));
observer.observe(document.querySelector(".showcase"));

// ---------------------------------------------------------------------------------------------
// Hero tagline types itself out.
const typed = document.querySelector(".typed");
if (typed && !reducedMotion) {
  const text = typed.dataset.type;
  typed.textContent = "";
  (async () => {
    await sleep(700);
    for (const ch of text) {
      typed.textContent += ch;
      await sleep(55 + Math.random() * 70);
    }
    await sleep(400);
    document.querySelector(".block-cursor").style.visibility = "hidden";
  })();
} else {
  document.querySelector(".block-cursor").style.visibility = "hidden";
}

// ---------------------------------------------------------------------------------------------
// Window preview: each pane plays a little script, in a loop, while it's on screen.
// The prompt from the screenshots: cwd, branch and the time on one line, `❯` on the next.
const clock = () => new Date().toTimeString().slice(0, 8);
const promptTop = () =>
  `<span class="pl"><span><span class="b">~/dev/nox</span> <span class="m">feat/stream*</span></span><span class="m">${clock()}</span></span>`;
const PS = '<span class="k">❯</span> ';

// Live agent state in the pane header and the tab's status dot (SPEC.md §4.2, §7.12).
const agentState = document.getElementById("agent-state");
const tabDot = document.getElementById("tab1-dot");
const mail = document.getElementById("mail");
let workingFor = null; // seconds, counted only while the animation runs
function setAgent(state) {
  workingFor = state === "working" ? 2 : null;
  agentState.classList.toggle("idle", state !== "working");
  agentState.textContent = state === "working" ? "working · 2s" : "idle";
  tabDot.className = state === "working" ? "tdot running" : "tdot";
}
setInterval(() => {
  if (workingFor === null || !running()) return;
  const s = ++workingFor;
  agentState.textContent = `working · ${s >= 60 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${s}s`}`;
}, 1000);

// Steps: ["cmd", text] a shell prompt + typed command, ["in", prefixHtml, text, cls] typed input,
// ["out", html] a line, ["wait", ms], ["agent", state], ["mail", n], ["clear"] ends the loop.
const SCRIPTS = {
  agent: [
    ["agent", "idle"],
    ["out", '<span class="ct">✳</span> <b>Claude Code</b> <span class="m">· ~/dev/nox</span>'],
    ["out", ""],
    ["in", PS, "stream PTY output in chunks, keep the tests green", "u"],
    ["agent", "working"],
    ["out", ""],
    ["wait", 500],
    ["out", "● I'll read the PTY reader first, then claim the crate."],
    ["out", ""],
    ["out", '<span class="m">  Read 2 files</span>'],
    ["wait", 700],
    ["out", ""],
    ["out", '● Bash(nox claim crates/nox_terminal/ -m "streaming")'],
    ["out", '<span class="m">  ⎿ </span><span class="g">claimed</span>'],
    ["wait", 900],
    ["out", ""],
    ["out", '● Update(pty.rs) <span class="g">+42</span> <span class="r">−17</span>'],
    ["wait", 700],
    ["out", '● Update(reader.rs) <span class="g">+18</span> <span class="r">−4</span>'],
    ["wait", 900],
    ["out", ""],
    ["out", "● Bash(nox spawn --split below -- cargo test)"],
    ["wait", 3200],
    ["out", '<span class="m">  ⎿ </span><span class="g">214 passed</span>'],
    ["out", ""],
    ["out", '● Bash(nox msg reviewer "streaming is ready")'],
    ["mail", 1],
    ["wait", 600],
    ["out", ""],
    ["out", "● Done: 2 files changed, all tests pass."],
    ["agent", "idle"],
    ["out", ""],
    ["in", PS, "", "u"],
    ["wait", 7000],
    ["clear"],
  ],
  cargo: [
    ["wait", 7600],
    ["cmd", "cargo test -p nox_terminal"],
    ["out", '   <span class="g"><b>Compiling</b></span> nox_terminal v0.1.2'],
    ["wait", 1100],
    ["out", '    <span class="g"><b>Finished</b></span> `test` profile in 3.41s'],
    ["out", '     <span class="g"><b>Running</b></span> unittests src/lib.rs'],
    ["wait", 300],
    ["out", 'test parser::utf8_split_chunks ... <span class="g">ok</span>'],
    ["out", 'test pty::reads_in_chunks ... <span class="g">ok</span>'],
    ["out", 'test grid::scroll_region ... <span class="g">ok</span>'],
    ["out", ""],
    ["out", 'test result: <span class="g">ok</span>. 214 passed; 0 failed'],
    ["cmd", ""],
    ["wait", 9000],
    ["clear"],
  ],
  shell: [
    ["cmd", "nox wait backend"],
    ["wait", 12600],
    ["cmd", "nox inbox"],
    ["out", '<span class="y">[backend]</span> streaming is ready'],
    ["mail", 0],
    ["wait", 500],
    ["cmd", "git diff --stat"],
    ["out", ' pty.rs    | 59 <span class="g">++++++</span><span class="r">---</span>'],
    ["out", ' reader.rs | 22 <span class="g">++++</span><span class="r">-</span>'],
    ["cmd", ""],
    ["wait", 6000],
    ["clear"],
  ],
};

// Pause / resume: time only passes while the window is on screen and not paused.
let paused = false;
let onScreen = false;
const running = () => onScreen && !paused;
async function tick(ms) {
  let left = ms;
  do {
    while (!running()) await sleep(150);
    const step = Math.min(left, 100);
    await sleep(step);
    left -= step;
  } while (left > 0);
}

const animToggle = document.querySelector(".anim-toggle");
animToggle.addEventListener("click", () => {
  paused = !paused;
  term.classList.toggle("paused", paused);
  animToggle.setAttribute("aria-pressed", String(paused));
  animToggle.querySelector("span").textContent = paused ? "play animation" : "pause animation";
});

// All panes restart together, so the story stays in order on every loop.
let arrived = 0;
let release;
let gate = new Promise((r) => (release = r));
function loopBarrier() {
  const current = gate;
  if (++arrived === panes.length) {
    arrived = 0;
    release();
    gate = new Promise((r) => (release = r));
  }
  return current;
}

class Pane {
  constructor(el, script) {
    this.el = el;
    this.script = script;
  }

  line(html, cls) {
    const span = document.createElement("span");
    span.className = cls ? `l ${cls}` : "l";
    span.innerHTML = html;
    this.el.append(span);
    this.trim();
    return span;
  }

  // Like a terminal: text starts at the top and scrolls up once the pane is full.
  trim() {
    while (this.el.children.length > 1 && this.el.scrollHeight > this.el.clientHeight + 1) {
      this.el.firstChild.remove();
    }
  }

  cursorTo(span) {
    this.el.querySelector(".cur")?.remove();
    const cur = document.createElement("span");
    cur.className = "cur";
    span.append(cur);
  }

  async type(prefix, text, cls) {
    const span = this.line(prefix, cls);
    const node = document.createTextNode("");
    span.insertBefore(node, null);
    this.cursorTo(span);
    span.insertBefore(node, span.lastChild);
    if (!text) return;
    await tick(600);
    for (const ch of text) {
      node.data += ch;
      this.trim();
      await tick(30 + Math.random() * 60);
    }
    await tick(300);
  }

  async run() {
    for (;;) {
      for (const [op, a, b, c] of this.script) {
        await tick(0);
        if (op === "cmd") {
          if (this.el.children.length) this.line("");
          this.line(promptTop());
          await this.type(PS, a);
        } else if (op === "in") await this.type(a, b, c);
        else if (op === "out") {
          this.el.querySelector(".cur")?.remove();
          this.line(a);
        } else if (op === "wait") await tick(a);
        else if (op === "agent") setAgent(a);
        else if (op === "mail") mail.hidden = a === 0;
        else if (op === "clear") {
          await loopBarrier();
          this.el.replaceChildren();
          mail.hidden = true;
        }
      }
    }
  }

  // Reduced motion: the final frame, without the typing.
  renderStatic() {
    for (const [op, a, b, c] of this.script) {
      if (op === "cmd") {
        if (this.el.children.length) this.line("");
        this.line(promptTop());
        this.line(PS + a);
      } else if (op === "in") this.line(a + b, c);
      else if (op === "out") this.line(a);
      else if (op === "agent") setAgent(a);
      else if (op === "clear") break;
    }
  }
}

const panes = [...document.querySelectorAll("[data-script]")].map(
  (el) => new Pane(el, SCRIPTS[el.dataset.script]),
);
if (reducedMotion) {
  panes.forEach((p) => p.renderStatic());
  animToggle.hidden = true;
} else {
  const termObserver = new IntersectionObserver((entries) => {
    onScreen = entries[0].isIntersecting;
  });
  termObserver.observe(term);
  panes.forEach((p) => p.run());
}

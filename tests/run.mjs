import { chromium, firefox } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const base = pathToFileURL(resolve("../dist/index.html")).href;
const url = base + "#fast,msh";
let failed = false;

const text = (p) => p.evaluate(() => document.getElementById("screen").innerText);

async function cmd(p, line) {
  await p.keyboard.type(line);
  await p.keyboard.press("Enter");
}

async function run(name, type) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  await p.goto(url);
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 8000 });
  const fails = [];
  const expect = async (label, ok) => { if (!ok) fails.push(label); };

  let t = await text(p);
  await expect("boot log", t.includes("kernel starting") && t.includes("OK") && t.includes("first boot"));
  await expect("login banner", t.includes("mavex login: user"));
  await expect("prompt", t.includes("user@mavex:~$"));

  await cmd(p, "echo hello 'big  world' $USER");
  await cmd(p, "mkdir -p proj/src && cd proj/src && pwd");
  await cmd(p, "echo data > f.txt && cat f.txt | wc -c");
  await cmd(p, "cd ~ && ls");
  await cmd(p, "ls /bin | head -n 2");
  await cmd(p, "nosuchcmd");
  await cmd(p, "echo $?");
  t = await text(p);
  await expect("echo", t.includes("hello big  world user"));
  await expect("cd/pwd", t.includes("/home/user/proj/src"));
  await expect("pipe/redir", /\n5\n/.test(t));
  await expect("ls", t.includes("proj"));
  await expect("not found", t.includes("msh: nosuchcmd: command not found") && /\n127\n/.test(t));

  // history
  await p.keyboard.press("ArrowUp");
  await p.keyboard.press("ArrowUp");
  await p.keyboard.press("Enter");
  t = await text(p);
  await expect("history", (t.match(/\n127\n/g) || []).length >= 1);

  // tab completion
  await p.keyboard.type("ls pro");
  await p.keyboard.press("Tab");
  await p.keyboard.type("src");
  await p.keyboard.press("Enter");
  t = await text(p);
  await expect("tab complete", t.includes("ls proj/src") && t.includes("f.txt"));
  await p.keyboard.type("whoa");
  await p.keyboard.press("Tab");
  await p.keyboard.press("Enter");
  t = await text(p);
  await expect("tab command", t.includes("whoami") && t.split("whoami").length >= 2);

  // line editing
  await p.keyboard.type("echo abc");
  await p.keyboard.press("ArrowLeft");
  await p.keyboard.press("Backspace");
  await p.keyboard.press("Enter");
  t = await text(p);
  await expect("editing", /\nac\n/.test(t));

  // ctrl-c and ctrl-l
  await p.keyboard.type("garbage");
  await p.keyboard.press("Control+c");
  t = await text(p);
  await expect("ctrl-c", t.includes("garbage^C"));
  await cmd(p, "clear");
  t = await text(p);
  await expect("clear", !t.includes("hello big") && /user@mavex:~\$/.test(t));

  // exit & restart
  await cmd(p, "exit");
  t = await text(p);
  await expect("exit", t.includes("process completed"));
  await p.keyboard.press("Enter");
  await cmd(p, "echo back");
  t = await text(p);
  await expect("restart", /\nback\n/.test(t));

  // real uutils coreutils (wasm32-wasip1) on the WASI bridge
  await cmd(p, "cd /");
  await cmd(p, "ls");
  await cmd(p, "ls /etc | cat");
  await cmd(p, "seq 1 100000 | wc -l");
  await cmd(p, "yes | head -n 2");
  await cmd(p, "cd ~/proj && echo hi | tee uu.txt | tr a-z A-Z && cat uu.txt && ls -l uu.txt");
  await cmd(p, "ls /nonexistent; echo rc=$?");
  t = await text(p);
  await expect("uutils ls columns", /bin\s+etc\s+home\s+proc\s+root\s+run\s+sys\s+tmp\s+usr\s+var/.test(t) && !/bin\netc\nhome/.test(t));
  await expect("uutils pipe", t.includes("hostname") && /\n100000\n/.test(t));
  await expect("yes|head", /\ny\ny\n/.test(t));
  await expect("tee/tr/cat", t.includes("HI") && /\nhi\n/.test(t) && /-rw\S+ \d+ \S+ \S+ 3 /.test(t));
  await expect("uutils error code", t.includes("ls: cannot access '/nonexistent'") && t.includes("rc=2"));

  // JS API
  const api = await p.evaluate(() => ({
    r: window.mavex.readFile("/home/user/proj/src/f.txt"),
    miss: window.mavex.readFile("/nope"),
  }));
  await expect("api", api.r === "data\n" && api.miss === -2);

  if (errs.length) fails.push("pageerror: " + errs.join("; "));
  console.log(`${fails.length ? "FAIL" : "PASS"} ${name}`, fails.length ? fails : "");
  if (fails.length) { failed = true; console.log((await text(p)).slice(-1500)); }
  await b.close();
}

async function runBrush(name, type) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  await p.goto(base + "#fast");
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 8000 });
  const fails = [];
  for (const l of ["id", "cat /etc/shadow", "sudo id", "ip link add d0 type dummy", "sudo ip link add d0 type dummy", "sudo ip addr add 10.1.2.3/24 dev d0", "sudo ip link set d0 up", "crabfetch --module-override localip", "wg selftest", "sudo mkdir -p /etc/wireguard", "printf '[Interface]\\nPrivateKey = YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\\nAddress = 10.9.0.2/24\\n[Peer]\\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\\nAllowedIPs = 10.9.0.0/24\\nEndpoint = 192.0.2.1:51820\\n' | sudo tee /etc/wireguard/wg0.conf", "sudo wg-quick up wg0", "wg show", "crabfetch --module-override localip", "ip -br addr", "ip route", "sudo -i", "id -u", "exit", "uname -r", "hostname", "history", "su", "echo a | tr a b"]) await cmd(p, l);
  await new Promise((r) => setTimeout(r, 500));
  // blocking behaviour (Asyncify): sleep, tty stdin, Ctrl-C, concurrent pipeline
  const ty = async (l) => { await p.keyboard.type(l); await p.keyboard.press("Enter"); };
  await ty("sleep 1; echo woke");
  await new Promise((r) => setTimeout(r, 300));
  const early = await text(p);
  await new Promise((r) => setTimeout(r, 1500));
  const late = await text(p);
  if (early.includes("\nwoke") || !late.includes("\nwoke")) fails.push("sleep blocks without freezing");
  await ty("cat");
  await new Promise((r) => setTimeout(r, 300));
  await ty("typed-input-line");
  await new Promise((r) => setTimeout(r, 300));
  await p.keyboard.press("Control+d");
  await new Promise((r) => setTimeout(r, 500));
  await ty("sleep 100");
  await new Promise((r) => setTimeout(r, 400));
  await p.keyboard.press("Control+c");
  await new Promise((r) => setTimeout(r, 400));
  await ty("echo after-ctrl-c");
  await ty("(sleep 0.3; echo slow-a) | cat");
  await new Promise((r) => setTimeout(r, 1200));
  await ty("systemctl is-active mavex-heartbeat; systemctl stop mavex-heartbeat; systemctl is-active mavex-heartbeat; systemctl start mavex-heartbeat; systemctl is-active mavex-heartbeat; journalctl -u mavex-heartbeat --no-pager | tail -n 3; journalctl --no-pager | head -n 8; systemctl list-units | head -n 6; cat /proc/1/comm");
  await new Promise((r) => setTimeout(r, 4000));
  await ty("crabfetch");
  await new Promise((r) => setTimeout(r, 2500));
  const alt = () => p.evaluate(() => ({ shown: getComputedStyle(document.getElementById("alt")).display !== "none", scr: getComputedStyle(document.getElementById("screen")).display !== "none", text: document.getElementById("alt").innerText }));
  await ty("htop");
  await new Promise((r) => setTimeout(r, 2500));
  let a = await alt();
  if (!a.shown || a.scr) fails.push("htop enters alternate screen");
  for (const w of ["CPU[", "Mem[", "Tasks:", "Command", "init", "brush", "F10Quit"]) if (!a.text.includes(w)) fails.push("htop shows " + w);
  await p.keyboard.press("ArrowDown"); await p.keyboard.press("F6");
  await new Promise((r) => setTimeout(r, 400));
  await p.keyboard.press("q");
  await new Promise((r) => setTimeout(r, 800));
  a = await alt();
  if (a.shown || !a.scr) fails.push("htop quit restores terminal");
  await ty("top");
  await new Promise((r) => setTimeout(r, 2500));
  a = await alt();
  for (const w of ["top - ", "Tasks:", "%Cpu(s)", "MiB Mem", "PID USER", "COMMAND"]) if (!a.text.includes(w)) fails.push("top shows " + w);
  await p.keyboard.press("q");
  await new Promise((r) => setTimeout(r, 800));
  await ty("top -b -n 1 | head -n 3; echo top-done");
  await new Promise((r) => setTimeout(r, 1500));
  const w = (ms) => new Promise((r) => setTimeout(r, ms));
  await ty("sleep 30 &"); await w(600);
  await ty("jobs; echo jc-listed"); await w(600);
  await ty("kill %1"); await w(600);
  await ty("jobs; echo jc-killed"); await w(600);
  await ty("sleep 30"); await w(600);
  await p.keyboard.press("Control+z"); await w(900);
  await ty("jobs; echo jc-stopped"); await w(600);
  await ty("kill -9 %1; kill -CONT %1; echo jc-pid=$$"); await w(1000);
  const jt = await text(p);
  if (!/Running\s+sleep 30\s+jc-listed/.test(jt.replace(/\n/g, " "))) fails.push("bg job listed Running");
  if (!/Stopped\s+sleep 30/.test(jt)) fails.push("ctrl-z stops foreground job");
  if (!/\njc-pid=\d+\n/.test(jt)) fails.push("$$ expands");
  if (/Running\s+sleep 30\s*\n\s*jc-killed/.test(jt)) fails.push("kill %1 ends bg job");
  await ty("stty -echo"); await w(500);
  await ty("read tx"); await w(500);
  await ty("hidden-pw-text"); await w(500);
  await ty("stty echo; echo tty-got=$tx; stty size; tty"); await w(1000);
  const st = await text(p);
  if (!st.includes("tty-got=hidden-pw-text")) fails.push("stty -echo input still read");
  if (/\nhidden-pw-text\n/.test(st)) fails.push("stty -echo hides typed text");
  if (!/\n\d+ \d+\n\/dev\/tty1\n/.test(st)) fails.push("stty size and tty");
  await p.keyboard.type("echo tabtest /tm"); await p.keyboard.press("Tab"); await w(500); await p.keyboard.press("Enter"); await w(800);
  await p.keyboard.type("echo zzz"); await p.keyboard.press("Control+r"); await p.keyboard.type("tabt"); await w(300); await p.keyboard.press("Enter"); await w(800);
  await p.keyboard.type("echo $PW"); await p.keyboard.press("Tab"); await w(500); await p.keyboard.press("Enter"); await w(800);
  const lt = await text(p);
  if ((lt.match(/\ntabtest \/tmp\/\n/g) || []).length !== 2) fails.push("tab completes path; ctrl-r runs history match");
  await ty("printf '\\033[?1049h\\033[?1000h\\033[?1006h\\033[?2004h\\033[5;5H'; cat -v; printf '\\033[?1049l'"); await w(1200);
  const ab = await p.locator("#alt").boundingBox();
  await p.mouse.click(ab.x + 100, ab.y + 60); await w(400);
  await p.keyboard.press("Enter"); await w(500);
  a = await alt();
  if (!/\^\[\[<0;\d+;\d+M/.test(a.text)) fails.push("mouse click reported in SGR mode: " + JSON.stringify(a.text.slice(0, 200)));
  await p.keyboard.press("Control+d"); await w(800);
  await p.keyboard.press("Control+Alt+t"); await w(2500);
  await ty("echo second-tab; tty"); await w(800);
  const t2 = await p.evaluate(() => [...document.querySelectorAll(".term")].map((t) => ({ on: t.classList.contains("on"), text: t.innerText })));
  if (t2.length !== 2 || !t2[1].on || t2[0].on) fails.push("new tab created and focused");
  if (!/\nsecond-tab\n\/dev\/tty2\n/.test(t2[1].text)) fails.push("second tab runs its own shell and tty");
  if (t2[0].text.includes("second-tab")) fails.push("tab output isolated");
  await ty("sleep 30 &"); await w(500);
  await p.keyboard.press("Alt+1"); await w(500);
  await ty("jobs; echo tab1-jobs-done"); await w(800);
  const t3 = await p.evaluate(() => [...document.querySelectorAll(".term")].map((t) => t.innerText));
  if (/sleep 30/.test(t3[0].split("tab1-jobs-done").slice(-2)[0].split("jobs; echo tab1")[1] || "")) fails.push("bg job leaked across tabs");
  await p.keyboard.press("Alt+2"); await w(300);
  await ty("exit"); await w(1500);
  const t4 = await p.evaluate(() => ({ n: document.querySelectorAll(".term").length, on: document.querySelector(".term.on").id }));
  if (t4.n !== 1 || t4.on !== "term0") fails.push("exit closes tab");
  if (!/\n\/home\/user\n/.test(lt.slice(lt.lastIndexOf("tabtest")))) fails.push("tab completes variable");
  const t = await text(p);
  if (!t.includes("top-done") || !t.includes("Tasks:")) fails.push("top -b batch mode and shell survives");
  if (!t.includes("heartbeat: system is alive")) fails.push("journal captures service output");
  if (!t.includes("Reached target") && !t.includes("Started MavexOS heartbeat")) fails.push("journal has init messages");
  if (!/\nactive\ninactive\nactive\n/.test(t.replace(/\n+/g, "\n"))) fails.push("systemctl start/stop cycle");
  if (!/Shell > brush/.test(t)) fails.push("crabfetch finds shell via /proc/self ppid");
  if (!t.includes("mavex-init")) fails.push("crabfetch names init system");
  if (!/Disk \(\/\) > [0-9.]+ [KMG]?B used of [0-9.]+ [KMG]B/.test(t)) fails.push("crabfetch disk space");
  if (!/Host > \S.* on \S/.test(t)) fails.push("crabfetch host from browser info");
  if (!/Display \(Browser Viewport\) > \d+x\d+/.test(t)) fails.push("crabfetch display");
  if (!/Terminal > mavex-terminal/.test(t)) fails.push("crabfetch terminal");
  if (!/Shell > brush 0\.5\.0/.test(t)) fails.push("crabfetch brush version");
  if ((t.match(/Locale >/g) || []).length !== 1) fails.push("single locale line");
  if (!/typed-input-line\ntyped-input-line\n/.test(t)) fails.push("cat reads tty");
  if (!t.includes("^C") || !t.includes("\nafter-ctrl-c\n")) fails.push("ctrl-c");
  if (!t.includes("\nslow-a\n")) fails.push("pipeline with sleep");
  const need = [["id", "uid=1000(user) gid=1000(user) groups=1000(user),27(sudo)"], ["shadow denied", "cat: /etc/shadow: Permission denied"], ["sudo", "uid=0(root)"], ["crabfetch localip", "Local IP (d0) > 10.1.2.3"], ["wg selftest", "boringtun OK"], ["wg show", "interface: wg0"], ["wg peer", "allowed ips: 10.9.0.0/24"], ["wg localip", "Local IP (wg0) > 10.9.0.2"], ["ip denied", "RTNETLINK answers: Operation not permitted"], ["ip addr", "d0               UNKNOWN"], ["ip route", "10.1.2.0/24 dev d0 proto kernel scope link src 10.1.2.3"], ["sudo -i", "root@mavex:~#"], ["uname", "\n0.0.17\n"], ["hostname", "\nmavex\n"], ["history", "  1  id"], ["su", "root@mavex:/home/user#"], ["pipe", "\nb\n"]];
  for (const [k, v] of need) if (!t.includes(v)) fails.push(k);
  await ty("seq 1 7000"); await w(3000);
  const sb = await p.evaluate(() => document.getElementById("out").textContent.split("\n").length);
  if (sb > 5200) fails.push("scrollback capped");
  if (errs.length) fails.push("pageerror: " + errs.join("; "));
  console.log(`${fails.length ? "FAIL" : "PASS"} ${name} (brush)`, fails.length ? fails : "");
  if (fails.length) { failed = true; console.log(t.slice(-5000)); }
  await b.close();
}

for (const [name, type] of [["chromium", chromium], ["firefox", firefox]]) { await run(name, type); await runBrush(name, type); }
process.exit(failed ? 1 : 0);

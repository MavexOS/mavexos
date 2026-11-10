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
  await expect("uutils ls columns", /bin\s+dev\s+etc\s+home\s+proc\s+root\s+run\s+sys\s+tmp\s+usr\s+var/.test(t) && !/bin\netc\nhome/.test(t));
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
  await ty("echo hello world > /tmp/sa; echo hallo world > /tmp/sb");
  await new Promise((r) => setTimeout(r, 400));
  await ty("sed s/world/SEDOK/ /tmp/sa");
  await new Promise((r) => setTimeout(r, 1200));
  await ty("diff /tmp/sa /tmp/sb");
  await new Promise((r) => setTimeout(r, 1500));
  await ty("mkdir -p /tmp/tt/d; echo tarok > /tmp/tt/d/f; cd /tmp; tar czf tt.tgz tt; mkdir tx; cd tx; tar xzf ../tt.tgz; cd /tmp");
  await new Promise((r) => setTimeout(r, 3500));
  await ty("echo p q r | awk '{print $2 NF}'");
  await new Promise((r) => setTimeout(r, 2500));
  await ty("find tx -name f | xargs cat");
  await new Promise((r) => setTimeout(r, 3500));
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
  await p.keyboard.press("F1"); await new Promise((r) => setTimeout(r, 500));
  a = await alt();
  if (!a.text.includes("htop (MavexOS own implementation)")) fails.push("htop F1 help");
  await p.keyboard.press("x"); await p.keyboard.press("F3"); await p.keyboard.type("init"); await new Promise((r) => setTimeout(r, 500));
  a = await alt();
  if (!a.text.includes("Search: init")) fails.push("htop F3 search");
  await p.keyboard.press("Enter"); await new Promise((r) => setTimeout(r, 300));
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
  await ty("seq 1 200 | less"); await w(1200);
  a = await alt();
  if (!a.shown || !/(^|\n)1 *\n2 *\n3 *\n/.test(a.text) || !/lines 1-/.test(a.text)) fails.push("less (piped) shows first page: " + JSON.stringify(a.text.slice(0, 120)));
  await p.keyboard.press("G"); await w(500);
  a = await alt();
  if (!/\n200 *\n/.test(a.text) || !/\(END\)/.test(a.text)) fails.push("less G goes to end");
  await p.keyboard.type("/150"); await p.keyboard.press("Enter"); await w(500); await p.keyboard.press("q"); await w(800);
  a = await alt();
  if (a.shown) fails.push("less q exits alt screen");
  await ty("man ls | head -3"); await w(1500);
  await ty("cd /tmp && edit e.txt"); await w(1500);
  await p.keyboard.type("hello editor"); await p.keyboard.press("Enter"); await p.keyboard.type("line two"); await p.keyboard.press("Control+s"); await w(500); await p.keyboard.press("Control+q"); await w(1000);
  await ty("cat e.txt; cd /home/user"); await w(800);
  const ed = await text(p);
  if (!/\nhello editor\nline two\n/.test(ed)) fails.push("edit saves file: " + JSON.stringify(ed.slice(-300)));
  if (!/No manual entry|LS|ls /.test(ed)) fails.push("man shows something");
  await ty("ln -s /tmp /home/user/lnk; realpath lnk/../x; realpath -m /a/b/../c; readlink -f lnk; readlink lnk; realpath -e /nonexistent; echo rc=$?"); await w(1200);
  await ty("passwd user"); await w(600); await ty("hunter2"); await w(300); await ty("hunter2"); await w(800);
  await ty("echo '%sudo ALL=(ALL) ALL' > /etc/sudoers; su user"); await w(1500);
  await ty("sudo id"); await w(800); await ty("wrong"); await w(1500); await ty("hunter2"); await w(1500);
  await ty("exit"); await w(800);
  await ty("echo '%sudo ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers; echo restored"); await w(800);
  const pw = await text(p);
  if (!/\n\/x\n\/a\/c\n\/tmp\n\/tmp\n/.test(pw) || !/realpath: \/nonexistent: No such file/.test(pw) || !/rc=1/.test(pw)) fails.push("realpath/readlink: " + JSON.stringify(pw.slice(-900)));
  if (!/password updated successfully/.test(pw) || !/\[sudo\] password for user: /.test(pw) || !/Sorry, try again\./.test(pw) || !/uid=0\(root\)/.test(pw.slice(pw.lastIndexOf("Sorry, try again."))) || /hunter2\n/.test(pw.replace(/passwd user\n/, "")) && /\nhunter2/.test(pw)) fails.push("passwd + sudo prompt: " + JSON.stringify(pw.slice(-900)));
  await ty("trap 'echo got-usr1' USR1; kill -USR1 $$; trap 'echo got-int' INT; echo bg-started"); await w(1200);
  await ty("sleep 31"); await w(600); await p.keyboard.press("Control+z"); await w(900);
  await ty("bg; echo bgdone"); await w(800);
  const tr = await text(p);
  if (!/\ngot-usr1\n/.test(tr)) fails.push("trap on real signal USR1: " + JSON.stringify(tr.slice(-600)));
  if (!/\[\d+\]\+? sleep 30 & sleep 31 &/.test(tr) && !/\[\d+\]\+? .*sleep.* &\n/.test(tr)) fails.push("bg prints job line: " + JSON.stringify(tr.slice(-600)));
  await ty("pkill sleep; trap - INT USR1"); await w(800);
  await ty("sleep 77 & sleep 78 &"); await w(600);
  await ty("ps | head -3; pgrep -x sleep | wc -l; pkill -f 'sleep 77'; sleep 0.5; pgrep -f 'sleep 77' || echo gone77; echo hi > in.txt; nohup echo nohup-ok; cat nohup.out; pkill sleep"); await w(2500);
  const ps = await text(p);
  if (!/PID TTY\s+TIME CMD/.test(ps) || !/\n\d+\n/.test(ps) || !/\ngone77\n/.test(ps) || !/nohup: ignoring input and appending output to 'nohup.out'/.test(ps) || !/\nnohup-ok\n/.test(ps)) fails.push("ps/pgrep/pkill/nohup: " + JSON.stringify(ps.slice(-500)));
  await p.keyboard.type("echo tabtest /tm"); await p.keyboard.press("Tab"); await w(500); await p.keyboard.press("Enter"); await w(800);
  await p.keyboard.type("echo zzz"); await p.keyboard.press("Control+r"); await p.keyboard.type("tabt"); await w(300); await p.keyboard.press("Enter"); await w(800);
  await p.keyboard.type("echo $PW"); await p.keyboard.press("Tab"); await w(500); await p.keyboard.press("Enter"); await w(800);
  const lt = await text(p);
  if ((lt.match(/\ntabtest \/tmp\/\n/g) || []).length !== 2) fails.push("tab completes path; ctrl-r runs history match");
  await ty("printf '\\033[?1049h\\033[?1000h\\033[?1006h\\033[?2004h\\033[5;5H'; cat -v; printf '\\033[?1049l'"); await w(1200);
  const ab = await p.locator("#alt").boundingBox() || (console.log("DBG", JSON.stringify((await text(p)).slice(-700))), {x:0,y:0});
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
  if (!t.includes("hello SEDOK")) fails.push("sed");
  if (!t.includes("\nq3\n")) fails.push("awk");
  if (!t.includes("\ntarok\n")) fails.push("tar roundtrip + find | xargs");
  if (!t.includes("< hello world") || !t.includes("> hallo world")) fails.push("diff");
  const need = [["id", "uid=1000(user) gid=1000(user) groups=1000(user),27(sudo)"], ["shadow denied", "cat: /etc/shadow: Permission denied"], ["sudo", "uid=0(root)"], ["crabfetch localip", "Local IP (d0) > 10.1.2.3"], ["wg selftest", "boringtun OK"], ["wg show", "interface: wg0"], ["wg peer", "allowed ips: 10.9.0.0/24"], ["wg localip", "Local IP (wg0) > 10.9.0.2"], ["ip denied", "RTNETLINK answers: Operation not permitted"], ["ip addr", "d0               UNKNOWN"], ["ip route", "10.1.2.0/24 dev d0 proto kernel scope link src 10.1.2.3"], ["sudo -i", "root@mavex:~#"], ["uname", "\n0.0.24\n"], ["hostname", "\nmavex\n"], ["history", "  1  id"], ["su", "root@mavex:/home/user#"], ["pipe", "\nb\n"]];
  for (const [k, v] of need) if (!t.includes(v)) fails.push(k);
  await ty("seq 1 7000"); await w(3000);
  const sb = await p.evaluate(() => document.getElementById("out").textContent.split("\n").length);
  if (sb > 5200) fails.push("scrollback capped");
  if (errs.length) fails.push("pageerror: " + errs.join("; "));
  console.log(`${fails.length ? "FAIL" : "PASS"} ${name} (brush)`, fails.length ? fails : "");
  if (fails.length) { failed = true; console.log(t.slice(-5000)); }
  await b.close();
}

async function runPersist(name, type) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  const fails = [];
  const boot = async () => { await p.goto(base + "#fast"); await p.waitForFunction(() => window.mavexReady, null, { timeout: 10000 }); };
  const ty = async (l) => { await p.keyboard.type(l); await p.keyboard.press("Enter"); };
  const w = (ms) => new Promise((r) => setTimeout(r, ms));
  await boot();
  await ty("echo keepme > ~/keep.txt && mkdir -p ~/d/e && chmod 600 ~/keep.txt && ln -s keep.txt ~/lnk && echo hi > /tmp/gone");
  await ty("printf 'bin\\x00\\xff\\x01' > ~/bin.dat");
  await w(3500);
  await p.reload();
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 10000 });
  await ty("cat ~/keep.txt; stat ~/keep.txt | grep Access; ls -d ~/d/e; readlink ~/lnk; wc -c < ~/bin.dat; cat /tmp/gone; ls /etc/passwd");
  await w(800);
  let t = await text(p);
  if (!t.includes("restored")) fails.push("restore message");
  if (!t.includes("keepme")) fails.push("file persisted");
  if (!t.includes("(0600/")) fails.push("mode persisted");
  if (!t.includes("/home/user/d/e")) fails.push("dir persisted");
  if (!t.includes("keep.txt\n")) fails.push("symlink persisted");
  if (!/\n6\n/.test(t)) fails.push("binary persisted");
  if (!t.includes("cat: /tmp/gone")) fails.push("tmp not persisted");
  await ty("rm ~/keep.txt");
  await w(3500);
  await p.reload();
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 10000 });
  await ty("cat ~/keep.txt");
  await w(500);
  t = await text(p);
  if (!t.includes("cat: /home/user/keep.txt")) fails.push("delete persisted");
  if (errs.length) fails.push("pageerror: " + errs.join("; "));
  console.log(`${fails.length ? "FAIL" : "PASS"} ${name} (persist)`, fails.length ? fails : "");
  if (fails.length) { failed = true; console.log(t.slice(-2000)); }
  await b.close();
}

async function runInit(name, type) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", (e) => errs.push(e.message));
  const fails = [];
  const ty = async (l) => { await p.keyboard.type(l); await p.keyboard.press("Enter"); };
  const w = (ms) => new Promise((r) => setTimeout(r, ms));
  await p.goto(base + "#fast");
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 10000 });
  await ty("printf '[Service]\\nType=oneshot\\nUser=user\\nExecStart=/bin/brush -c \"id -un > /tmp/svcuser\"\\n' | sudo tee /etc/systemd/system/whoami.service > /dev/null");
  await ty("sudo systemctl start whoami; sleep 1; cat /tmp/svcuser; rm /tmp/svcuser");
  await w(3000);
  await ty("printf '[Timer]\\nOnActiveSec=1s\\nUnit=whoami.service\\n' | sudo tee /etc/systemd/system/t1.timer > /dev/null; sudo systemctl start t1.timer; sleep 3; sudo systemctl list-timers; echo TIMER; cat /tmp/svcuser");
  await w(6000);
  await ty("echo rebooted > ~/rb; reboot");
  await w(1500);
  const pre = await text(p);
  await ty("sudo reboot");
  await w(6000);
  await p.waitForFunction(() => window.mavexReady, null, { timeout: 10000 });
  await ty("cat ~/rb");
  await w(800);
  const t = pre + (await text(p));
  if (!t.includes("\nuser\n")) fails.push("User= service ran as user");
  if (!t.includes("t1.timer") || !t.includes("whoami.service")) fails.push("list-timers");
  if (!/TIMER\nuser\n/.test(t)) fails.push("timer triggered service");
  if (!t.includes("Access denied")) fails.push("reboot denied for non-root");
  if (!/\nrebooted\n/.test(t)) fails.push("reboot kept ~ after reload");
  if (errs.length) fails.push("pageerror: " + errs.join("; "));
  console.log(`${fails.length ? "FAIL" : "PASS"} ${name} (init)`, fails.length ? fails : "");
  if (fails.length) { failed = true; console.log(t.slice(-2500)); }
  await b.close();
}

for (const [name, type] of [["chromium", chromium], ["firefox", firefox]]) { await run(name, type); await runBrush(name, type); await runPersist(name, type); await runInit(name, type); }
process.exit(failed ? 1 : 0);

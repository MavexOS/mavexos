import { chromium, firefox } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const url = pathToFileURL(resolve("../dist/index.html")).href;
let failed = false;

async function check(name, p) {
  const out = await p.textContent("#term");
  const fs = await p.evaluate(() => {
    const m = window.mavex;
    return {
      root: m.readdir("/"),
      host: m.readFile("/etc/hostname"),
      mk: m.mkdir("/home/user/docs"),
      dup: m.mkdir("/home/user/docs"),
      w: m.writeFile("/home/user/docs/a.txt", "héllo wörld"),
      r: m.readFile("/home/user/docs/a.txt"),
      ls: m.readdir("/home/user/docs"),
      missing: m.readFile("/nope"),
      rm: m.unlink("/home/user/docs/a.txt"),
      lsAfter: m.readdir("/home/user/docs"),
    };
  });
  const checks = {
    boot: out.includes("MavexOS kernel booting") && out.includes("vfs mounted"),
    root: JSON.stringify(fs.root) === JSON.stringify(["bin", "etc", "home", "tmp", "usr"]),
    host: fs.host === "mavex\n",
    mkdir: fs.mk === 0 && fs.dup === -17,
    rw: fs.w === 0 && fs.r === "héllo wörld" && JSON.stringify(fs.ls) === '["a.txt"]',
    enoent: fs.missing === -2,
    unlink: fs.rm === 0 && fs.lsAfter.length === 0,
  };
  const bad = Object.entries(checks).filter(([, v]) => !v).map(([k]) => k);
  console.log(`${bad.length ? "FAIL" : "PASS"} ${name}`, bad.length ? bad : "");
  if (bad.length) { failed = true; console.log(JSON.stringify(fs)); }
}

for (const [name, type] of [["chromium", chromium], ["firefox", firefox]]) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", e => errs.push(e.message));
  await p.goto(url);
  await p.waitForFunction(() => window.mavex, null, { timeout: 5000 }).catch(() => {});
  if (errs.length) { failed = true; console.log("FAIL", name, errs); }
  else await check(name, p);
  await b.close();
}
process.exit(failed ? 1 : 0);

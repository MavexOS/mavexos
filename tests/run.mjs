import { chromium, firefox } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const url = pathToFileURL(resolve("../dist/index.html")).href;
let failed = false;
for (const [name, type] of [["chromium", chromium], ["firefox", firefox]]) {
  const b = await type.launch();
  const p = await b.newPage();
  const errs = [];
  p.on("pageerror", e => errs.push(e.message));
  await p.goto(url);
  await p.waitForFunction(() => document.getElementById("term").textContent.includes("mavex$"), null, { timeout: 5000 }).catch(() => {});
  const out = await p.textContent("#term");
  const ok = out.includes("MavexOS kernel booting") && errs.length === 0;
  console.log(`${ok ? "PASS" : "FAIL"} ${name}`, errs.length ? errs : "");
  if (!ok) { failed = true; console.log(JSON.stringify(out)); }
  await b.close();
}
process.exit(failed ? 1 : 0);

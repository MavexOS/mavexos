import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
const base = pathToFileURL(resolve("../dist/index.html")).href;
const steps = JSON.parse(process.argv[2]);
const b = await chromium.launch(); const p = await b.newPage();
p.on("console", (m) => { if (m.text().startsWith("TRAP")) console.log(m.text().slice(0, 1500)); }); p.on("pageerror", (e) => console.log("PAGEERR", e.message));
await p.goto(base + "#fast");
await p.waitForFunction(() => window.mavexReady, null, { timeout: 15000 });
const text = () => p.evaluate(() => document.getElementById("screen").innerText);
const alt = () => p.evaluate(() => document.getElementById("alt").innerText);
for (const s of steps) {
  if (typeof s === "number") await new Promise((r) => setTimeout(r, s));
  else if (s.startsWith("^")) await p.keyboard.press("Control+" + s.slice(1));
  else if (s.startsWith("@")) await p.keyboard.press(s.slice(1));
  else if (s.startsWith("~")) await p.keyboard.type(s.slice(1));
  else if (s === "?alt") console.log("ALT>>\n" + await alt());
  else { await p.keyboard.type(s); await p.keyboard.press("Enter"); }
}
await new Promise((r) => setTimeout(r, 300));
console.log((await text()).slice(-2500));
await b.close();

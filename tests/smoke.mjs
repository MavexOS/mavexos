import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
const url = pathToFileURL(resolve("../dist/index.html")).href + "#fast";
const b = await chromium.launch(); const p = await b.newPage();
p.on("pageerror", (e) => console.log("PAGEERR", e.message));
await p.goto(url);
await p.waitForFunction(() => window.mavexReady, null, { timeout: 15000 });
for (const l of process.argv.slice(2)) { await p.keyboard.type(l); await p.keyboard.press("Enter"); }
await new Promise(r => setTimeout(r, 1500));
console.log((await p.evaluate(() => document.getElementById("screen").innerText)).slice(-1800));
await b.close();

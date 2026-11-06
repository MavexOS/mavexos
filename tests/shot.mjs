import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1000, height: 560 } });
await p.goto(pathToFileURL(resolve("../dist/index.html")).href + "#fast");
await p.waitForFunction(() => window.mavexReady, null, { timeout: 8000 });
await new Promise(r => setTimeout(r, 1500));
for (const c of ["htop", "top"]) {
  await p.keyboard.type(c); await p.keyboard.press("Enter");
  await new Promise(r => setTimeout(r, 3000));
  await p.screenshot({ path: `../.scratch/${c}.png` });
  await p.keyboard.press("q"); await new Promise(r => setTimeout(r, 800));
}
await b.close();

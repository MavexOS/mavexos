// Instruments a wasm32-wasip1 app with binaryen's Asyncify so host imports can suspend the process.
// Usage: node tools/asyncify.mjs in.wasm out.wasm
import binaryen from "binaryen";
import { readFileSync, writeFileSync } from "node:fs";
const [inp, out] = process.argv.slice(2);
const m = binaryen.readBinary(readFileSync(inp));
const F = binaryen.Features;
m.setFeatures(F.MVP | F.MutableGlobals | F.NontrappingFPToInt | F.SignExt | F.BulkMemory | F.BulkMemoryOpt | F.CallIndirectOverlong | F.Multivalue | F.ReferenceTypes | F.SIMD128 | F.Atomics | F.ExtendedConst);
binaryen.setPassArgument("asyncify-imports", [
  "wasi_snapshot_preview1.fd_read", "wasi_snapshot_preview1.fd_write", "wasi_snapshot_preview1.poll_oneoff",
  "wasi_snapshot_preview1.sched_yield", "mavex.wait", "mavex.wait_change", "mavex.fetch_wait",
].join(","));
m.runPasses(["asyncify"]);
writeFileSync(out, m.emitBinary());

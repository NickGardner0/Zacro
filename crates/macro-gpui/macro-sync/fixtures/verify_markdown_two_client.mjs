import { readFileSync } from "node:fs";
import { LoroDoc } from "loro-crdt";

const fixture = JSON.parse(readFileSync(new URL(
  "./markdown_two_client_typescript.json",
  import.meta.url,
)));
const bytes = (name) => Buffer.from(fixture[name], "hex");
const nativeUpdateArgument = process.argv.find((argument) =>
  argument.startsWith("--native-update-hex="),
);
const nativeUpdateHex = nativeUpdateArgument?.slice(
  "--native-update-hex=".length,
) ?? fixture.native_update_hex;
if (!/^(?:[0-9a-fA-F]{2})+$/.test(nativeUpdateHex)) {
  throw new Error("--native-update-hex must contain complete hexadecimal bytes");
}

const document = new LoroDoc();
document.import(bytes("seed_snapshot_hex"));
document.import(Buffer.from(nativeUpdateHex, "hex"));
document.import(bytes("browser_update_hex"));

const actual = document.toJSON();
if (process.argv.includes("--emit-converged")) {
  console.log(JSON.stringify(actual, null, 2));
  process.exit(0);
}
if (JSON.stringify(actual) !== JSON.stringify(fixture.expected_converged)) {
  console.error(JSON.stringify(actual, null, 2));
  throw new Error("JavaScript/WASM convergence no longer matches the fixture");
}
console.log(
  nativeUpdateArgument
    ? "JavaScript/WASM imported a live Rust update and converged"
    : "JavaScript/WASM imported the recorded Rust update and converged",
);

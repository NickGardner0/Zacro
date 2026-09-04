import { LoroDoc, LoroMap, LoroMovableList, LoroText } from "loro-crdt";

const hex = (bytes) => Buffer.from(bytes).toString("hex");

function insertTextNode(parent, id, text) {
  const node = parent.pushContainer(new LoroMap());
  node.setContainer("$", new LoroMap()).set("id", id);
  node.set("type", "text");
  node.set("version", 1);
  node.set("format", 0);
  node.set("detail", 0);
  node.set("mode", "normal");
  node.set("style", "");
  const value = node.setContainer("text", new LoroText());
  value.insert(0, text);
  return value;
}

const document = new LoroDoc();
document.setPeerId(701n);
document.setRecordTimestamp(true);
const root = document.getMap("root");
root.set("type", "root");
root.set("version", 1);
root.set("direction", null);
root.set("format", "");
root.set("indent", 0);
root.setContainer("$", new LoroMap()).set("id", "root");
const blocks = root.setContainer("children", new LoroMovableList());

const paragraph = blocks.pushContainer(new LoroMap());
paragraph.set("type", "paragraph");
paragraph.set("version", 1);
paragraph.set("direction", null);
paragraph.set("format", "");
paragraph.set("indent", 0);
paragraph.set("textFormat", 0);
paragraph.set("textStyle", "");
paragraph.setContainer("$", new LoroMap()).set("id", "paragraph-js");
const paragraphChildren = paragraph.setContainer("children", new LoroMovableList());
const paragraphText = insertTextNode(paragraphChildren, "text-js", "Shared seed");

const heading = blocks.pushContainer(new LoroMap());
heading.set("type", "heading");
heading.set("tag", "h2");
heading.set("version", 1);
heading.set("direction", null);
heading.set("format", "");
heading.set("indent", 0);
heading.setContainer("$", new LoroMap()).set("id", "heading-js");
const headingChildren = heading.setContainer("children", new LoroMovableList());
insertTextNode(headingChildren, "heading-text-js", "Browser heading");

const checklist = blocks.pushContainer(new LoroMap());
checklist.set("type", "list");
checklist.set("listType", "check");
checklist.set("start", 1);
checklist.set("tag", "ul");
checklist.set("version", 1);
checklist.set("direction", null);
checklist.set("format", "");
checklist.set("indent", 0);
checklist.setContainer("$", new LoroMap()).set("id", "checklist-js");
const checklistChildren = checklist.setContainer("children", new LoroMovableList());
const checklistItem = checklistChildren.pushContainer(new LoroMap());
checklistItem.set("type", "listitem");
checklistItem.set("checked", false);
checklistItem.set("value", 1);
checklistItem.set("version", 1);
checklistItem.set("direction", null);
checklistItem.set("format", "");
checklistItem.set("indent", 0);
checklistItem.setContainer("$", new LoroMap()).set("id", "checklist-item-js");
const checklistItemChildren = checklistItem.setContainer(
  "children",
  new LoroMovableList(),
);
insertTextNode(checklistItemChildren, "checklist-text-js", "Preserve structured task");

// This intentionally models a node introduced by a newer Macro client. Rust
// must relay it byte-for-byte while editing known siblings.
const future = blocks.pushContainer(new LoroMap());
future.set("type", "future-widget");
future.set("version", 7);
future.set("payload", "opaque-payload");
future.set("enabled", true);
future.setContainer("$", new LoroMap()).set("id", "future-widget-js");
const futureChildren = future.setContainer("children", new LoroMovableList());
insertTextNode(futureChildren, "future-text-js", "Future client content");

document.commit();
const seedSnapshot = document.export({ mode: "snapshot" });
const seedVersion = document.version();
paragraphText.insert(paragraphText.length, " from browser");
document.commit();

console.log(JSON.stringify({
  seed_snapshot_hex: hex(seedSnapshot),
  browser_update_hex: hex(document.export({ mode: "update", from: seedVersion })),
  coverage: ["paragraph", "heading", "checklist", "unknown_opaque_block"],
  expected_after_browser_update: document.toJSON(),
}, null, 2));

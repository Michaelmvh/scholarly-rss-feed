const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const script = fs.readFileSync(
  path.join(__dirname, "../src/assets/reader.js"),
  "utf8",
);

function element(properties = {}) {
  return {
    listeners: {},
    addEventListener(event, listener) {
      this.listeners[event] = listener;
    },
    ...properties,
  };
}

function reader({ checked = [true, false], authors = true, mode = "default" } = {}) {
  const checkboxes = checked.map((checked, index) =>
    element({ checked, dataset: { label: `Option ${index + 1}` } }),
  );
  const allLabel = { hidden: true };
  const selectAll = authors
    ? element({ checked: false, closest: () => allLabel })
    : null;
  const authorMode = authors ? { value: mode } : null;
  const authorSelection = authors ? { disabled: false } : null;
  const summary = { dataset: {}, textContent: "" };
  const picker = element({
    open: true,
    querySelector(selector) {
      return {
        "[data-select-all]": selectAll,
        'input[name="view_authors"]': authorMode,
        'input[name="view_author_selection"]': authorSelection,
        "[data-selection-summary]": summary,
      }[selector];
    },
    querySelectorAll: () => checkboxes,
  });
  let submissions = 0;
  const form = {
    classList: { add() {} },
    requestSubmit() {
      submissions += 1;
    },
    querySelectorAll(selector) {
      return selector === "[data-auto-submit]" ? [] : [picker];
    },
  };
  const document = element({ querySelector: () => form });
  vm.runInNewContext(script, { document });
  return {
    checkboxes, selectAll, authorMode, authorSelection, summary, allLabel,
    close() {
      picker.open = false;
      picker.listeners.toggle();
      return submissions;
    },
  };
}

test("initial author defaults stay selected without submitting", () => {
  const page = reader();
  assert.deepEqual(page.checkboxes.map((box) => box.checked), [true, false]);
  assert.equal(page.authorMode.value, "default");
  assert.equal(page.authorSelection.disabled, false);
  assert.equal(page.summary.textContent, "Option 1");
  assert.equal(page.selectAll.checked, false);
  assert.equal(page.selectAll.indeterminate, true);
  assert.equal(page.allLabel.hidden, false);
  assert.equal(page.close(), 0);
});

test("select all includes optional authors and submits when closed", () => {
  const page = reader();
  page.selectAll.checked = true;
  page.selectAll.listeners.change();
  assert.deepEqual(page.checkboxes.map((box) => box.checked), [true, true]);
  assert.equal(page.authorMode.value, "all");
  assert.equal(page.authorSelection.disabled, true);
  assert.equal(page.summary.textContent, "All tracked authors");
  assert.equal(page.selectAll.indeterminate, false);
  assert.equal(page.close(), 1);
});

test("deselect all creates an explicit empty selection", () => {
  const page = reader({ checked: [true, true] });
  page.selectAll.checked = false;
  page.selectAll.listeners.change();
  assert.deepEqual(page.checkboxes.map((box) => box.checked), [false, false]);
  assert.equal(page.authorMode.value, "custom");
  assert.equal(page.summary.textContent, "No tracked authors");
  assert.equal(page.close(), 1);
});

test("individual author changes switch from defaults to a custom selection", () => {
  const page = reader();
  page.checkboxes[0].checked = false;
  page.checkboxes[0].listeners.change();
  assert.equal(page.authorMode.value, "custom");
  assert.equal(page.authorSelection.disabled, true);
  assert.equal(page.selectAll.checked, false);
  assert.equal(page.summary.textContent, "No tracked authors");
  assert.equal(page.close(), 1);
});

test("unchanged all selection keeps its mode and original fingerprint", () => {
  const page = reader({ checked: [true, true], mode: "all" });
  assert.equal(page.authorMode.value, "all");
  assert.equal(page.authorSelection.disabled, false);
  assert.equal(page.close(), 0);
});

test("editing all selection switches to custom and drops the old fingerprint", () => {
  const page = reader({ checked: [true, true], mode: "all" });
  page.checkboxes[0].checked = false;
  page.checkboxes[0].listeners.change();
  assert.equal(page.authorMode.value, "custom");
  assert.equal(page.authorSelection.disabled, true);
  assert.equal(page.close(), 1);
});

test("checking the last author explicitly selects all", () => {
  const page = reader();
  page.checkboxes[1].checked = true;
  page.checkboxes[1].listeners.change();
  assert.equal(page.authorMode.value, "all");
  assert.equal(page.authorSelection.disabled, true);
  assert.equal(page.close(), 1);
});

test("all-optional defaults do not implicitly select all", () => {
  const page = reader({ checked: [false, false] });
  assert.equal(page.selectAll.checked, false);
  assert.equal(page.authorMode.value, "default");
  assert.equal(page.summary.textContent, "No tracked authors");
  assert.equal(page.close(), 0);
});

test("paper sources still require at least one selection", () => {
  const page = reader({ checked: [true, false], authors: false });
  page.checkboxes[0].checked = false;
  page.checkboxes[0].listeners.change();
  assert.equal(page.checkboxes[0].checked, true);
  assert.equal(page.close(), 0);
});

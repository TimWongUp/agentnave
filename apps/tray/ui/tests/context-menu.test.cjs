const test = require("node:test");
const assert = require("node:assert/strict");
const { common } = require("./helpers/common.cjs");

test("WebKit's menu stays for text entry and selected text under the pointer, and is blocked elsewhere", () => {
  const { menuBlocked } = common();
  const input = type => ({ closest: () => ({ type }) });
  const selected = { getRangeAt: () => ({ getClientRects: () => [{ left: 10, right: 50, top: 10, bottom: 20 }] }), rangeCount: 1, toString: () => "words" };
  assert.equal(menuBlocked({ target: input("text") }), false);
  assert.equal(menuBlocked({ target: { closest: () => ({ type: "textarea" }) } }), false);
  assert.equal(menuBlocked({ target: { closest: () => null, isContentEditable: true } }), false);
  assert.equal(menuBlocked({ target: input("checkbox") }), true);
  assert.equal(menuBlocked({ target: { closest: () => null, isContentEditable: false } }), true);
  assert.equal(menuBlocked({ target: { closest: () => null }, clientX: 20, clientY: 15 }, selected), false);
  assert.equal(menuBlocked({ target: { closest: () => null }, clientX: 80, clientY: 15 }, selected), true);
  // A menu from the keyboard has no pointer position: a selection keeps it, none does not.
  assert.equal(menuBlocked({ target: { closest: () => null } }, selected), false);
  assert.equal(menuBlocked({ target: { closest: () => null } }), true);
});

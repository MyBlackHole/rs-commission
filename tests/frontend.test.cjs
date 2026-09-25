// No npm dependencies. Tests execute the actual frontend money functions.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const context = vm.createContext({
  document: { getElementById: () => ({ addEventListener() {} }) },
  console, Date, BigInt, Intl,
});
const source = fs.readFileSync(path.join(__dirname, '../web/app.js'), 'utf8');
vm.runInContext(`${source}\nglobalThis.testExports = {money, minor};`, context);
const {money, minor} = context.testExports;

test('yuan input is parsed as integer cents without floats', () => {
  assert.equal(minor('0.01'), '1');
  assert.equal(minor('123.4'), '12340');
  assert.equal(minor('1000000000000.00'), '100000000000000');
});
test('ambiguous, scientific, signed and over-precision input is rejected', () => {
  for (const bad of ['-1','1.001','1e3','+1','','NaN','1,000']) assert.throws(() => minor(bad));
});
test('single-operation upper bound is enforced', () => {
  assert.throws(() => minor('1000000000000.01'));
});
test('formatting retains cents beyond Number.MAX_SAFE_INTEGER', () => {
  assert.equal(money('9007199254740993'), '¥90,071,992,547,409.93');
});
test('negative balance is explicitly displayed', () => {
  assert.equal(money('-150'), '-¥1.50');
  assert.equal(money('0'), '¥0.00');
});

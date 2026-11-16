const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const m = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(__dirname, '../Model.js'), 'utf8'), m);
const status = () => ({connected:true, profile:'quiet', calibrating:false, fans:[{label:'CPU',rpm:1000,percent:30,managed:true}],temperatures:[{label:'CPU',value:45}],warnings:[]});
test('validates local status and preserves text literally for PlainText sinks', () => {
  const d = status(); d.fans[0].label = '<img src="https://example.invalid/tracker">';
  assert.equal(m.parseStatus(JSON.stringify(d)).fans[0].label, d.fans[0].label);
  assert.equal(m.parseStatus('{oops'), null);
  d.profile = 'arbitrary'; assert.equal(m.parseStatus(JSON.stringify(d)), null);
});
test('rejects excessive data and invalid numeric readings', () => {
  assert.equal(m.parseStatus(' '.repeat(65537)), null);
  const d = status(); d.fans = Array(33).fill(d.fans[0]); assert.equal(m.parseStatus(JSON.stringify(d)), null);
  d.fans = [{rpm:-1, percent:1000}]; const p = m.parseStatus(JSON.stringify(d));
  assert.equal(p.fans[0].rpm, null); assert.equal(p.fans[0].percent, null);
});
test('shared tooltip and button text cannot be interpreted as markup', () => {
  const text = '<img src="https://example.invalid/tracker">&lt;b&gt;';
  assert.doesNotMatch(m.sharedText(text), /[<>&]/);
  assert.equal(m.sharedText('Airbend · Quiet'), 'Airbend · Quiet');
});
test('socket paths remain argv values and invalid paths fall back safely', () => {
  assert.equal(m.socketPath('/tmp/a socket;echo x'), '/tmp/a socket;echo x');
  assert.equal(m.socketPath('/tmp/bad\npath'), '/run/airbend/airbend.sock');
  assert.equal(m.socketPath('relative'), '/run/airbend/airbend.sock');
});
test('Open Airbend matches the app rather than a project terminal title', () => {
  const command = m.launchCommand();
  assert.equal(command[0], 'omarchy-launch-or-focus');
  assert.equal(command[2], 'airbend');
  const pattern = new RegExp('\\b' + command[1] + '\\b', 'i');
  assert.ok(pattern.test('airbend'));
  assert.ok(!pattern.test('Profiles and tray work | airbend'));
  assert.ok(!pattern.test('developer@workstation:~/src/airbend'));
});

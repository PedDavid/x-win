// Live integration: outputs only counts, IDs, booleans and timings, never titles/paths.
// Pass a freshly built .node file explicitly; never loads an installed platform package.
const assert = require('node:assert/strict')
const { resolve } = require('node:path')
const { performance } = require('node:perf_hooks')
const artifact = resolve(process.argv[2] || 'target/debug/x-win.node')
const xwin = require(artifact)

const active = xwin.activeWindow()
const windows = xwin.openWindows()
assert.equal(active.os, 'linux')
assert.equal(new Set(windows.map((w) => w.id)).size, windows.length)
assert.ok(windows.every((w) => w.id !== 0))
assert.equal(active.getIcon().data, '')
active.toObject() // Validate empty icons and unsupported URLs don't break this consumer.
const samples = []
for (let i = 0; i < 100; i++) {
  const start = performance.now()
  xwin.activeWindow()
  samples.push(performance.now() - start)
}
samples.sort((a, b) => a - b)
console.log(JSON.stringify({ artifact, focused: active.id !== 0, windowCount: windows.length,
  queryMs: { median: samples[50], p95: samples[95], max: samples[99] } }))
let callbacks = 0
let errors = 0
const cpuStart = process.cpuUsage()
const subscribeStart = performance.now()
const subscription = xwin.subscribeActiveWindow((error, window) => {
  if (error) { errors++; return }
  callbacks++
  assert.ok(window)
}, 100)
setTimeout(() => {
  xwin.unsubscribeActiveWindow(subscription)
  const cpu = process.cpuUsage(cpuStart)
  const subscribedMs = performance.now() - subscribeStart
  const pollingCpuPercent = (cpu.user + cpu.system) / (subscribedMs * 10)
  // Exercise repeated lifecycle without printing any window metadata.
  for (let i = 0; i < 10; i++) {
    const id = xwin.subscribeActiveWindow(() => {}, 60_000)
    xwin.unsubscribeActiveWindow(id)
  }
  xwin.unsubscribeAllActiveWindow()
  console.log(JSON.stringify({ callbacks, errors, subscribedMs, pollingCpuPercent }))
  assert.ok(callbacks > 0)
}, 1500)

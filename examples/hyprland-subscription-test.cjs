// Compositor-independent native Node integration. No npm dependencies required.
const assert = require('node:assert/strict')
const { Worker, isMainThread, parentPort, workerData } = require('node:worker_threads')
const { mkdtempSync, mkdirSync, rmSync } = require('node:fs')
const { tmpdir } = require('node:os')
const { join, resolve } = require('node:path')
const { setTimeout: sleep } = require('node:timers/promises')

if (!isMainThread) {
  const net = require('node:net')
  let state = { title: 'Synthetic title A', focus: true, failure: false }
  const client = (address) => ({ address, pid: 2147483647, class: 'fixture.Editor', title: state.title,
    at: [-10, 20], size: [800, 600], fullscreen: 2 })
  const server = net.createServer((socket) => {
    socket.once('data', (request) => {
      if (state.failure) { socket.end('invalid JSON'); return }
      if (request.toString() === 'j/clients') {
        socket.end(JSON.stringify([client('0x100000001'), client('0x200000001')]))
      } else if (request.toString() === 'j/activewindow') {
        socket.end(JSON.stringify(state.focus ? client('0x100000001') : {}))
      } else {
        socket.end('invalid request')
      }
    })
    socket.on('error', () => {})
  })
  parentPort.on('message', (next) => { Object.assign(state, next); parentPort.postMessage('updated') })
  server.listen(workerData.socket, () => parentPort.postMessage('ready'))
} else {
  async function main() {
    const dir = mkdtempSync(join(tmpdir(), 'x-win-node-'))
    mkdirSync(join(dir, 'hypr', 'fixture'), { recursive: true })
    const worker = new Worker(__filename, { workerData: { socket: join(dir, 'hypr', 'fixture', '.socket.sock') } })
    const message = () => new Promise((resolve) => worker.once('message', resolve))
    const update = async (state) => { const ack = message(); worker.postMessage(state); await ack }
    let xwin
    try {
      await message()
      process.env.XDG_RUNTIME_DIR = dir
      process.env.HYPRLAND_INSTANCE_SIGNATURE = 'fixture'
      process.env.XDG_CURRENT_DESKTOP = 'Hyprland'
      const artifact = resolve(process.argv[2] || 'target/debug/x-win.node')
      xwin = require(artifact)
      const windows = xwin.openWindows()
      assert.equal(windows.length, 2)
      assert.notEqual(windows[0].id, windows[1].id)
      assert.equal(windows[0].info.processId, windows[1].info.processId)
      assert.equal(windows[0].info.path, '') // process has exited / unavailable
      assert.equal(windows[0].usage.memory, 0)
      const events = []
      const id = xwin.subscribeActiveWindow((error, info) => events.push(error ? { error: true } : { id: info.id, title: info.title }), 10)
      async function until(predicate) {
        const deadline = Date.now() + 2000
        while (!predicate()) {
          assert.ok(Date.now() < deadline, 'Timed out waiting for subscription event')
          await sleep(10)
        }
      }
      await until(() => events.some((e) => e.title === 'Synthetic title A'))
      const initialId = events[0].id
      await update({ title: 'Synthetic title B' })
      await until(() => events.some((e) => e.title === 'Synthetic title B'))
      assert.equal(events.find((e) => e.title === 'Synthetic title B').id, initialId)
      await update({ focus: false })
      await until(() => events.some((e) => e.id === 0))
      const checkpoint = events.length
      await update({ focus: true })
      await until(() => events.length > checkpoint && events.at(-1).id === initialId)
      await update({ failure: true })
      await until(() => events.some((e) => e.error))
      assert.equal(events.at(-2).id, 0) // invalidate previous activity before error
      await update({ failure: false })
      await until(() => !events.at(-1).error && events.at(-1).id > initialId)
      xwin.unsubscribeActiveWindow(id)
      await sleep(200)
      const stopped = events.length
      await update({ title: 'Synthetic title C' })
      await sleep(150)
      assert.equal(events.length, stopped)
      for (let i = 0; i < 10; i++) {
        const subscription = xwin.subscribeActiveWindow(() => {}, 60_000)
        xwin.unsubscribeActiveWindow(subscription)
      }
      xwin.unsubscribeAllActiveWindow()
      console.log('PASS: native snapshots, shared PID identity, initial focus, title-only, no focus, restoration, retry, cancellation, repeated lifecycle')
    } finally {
      xwin?.unsubscribeAllActiveWindow()
      await worker.terminate()
      rmSync(dir, { recursive: true, force: true })
    }
  }
  main().catch((error) => { console.error(error); process.exitCode = 1 })
}

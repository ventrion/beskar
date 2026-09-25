# CDP cookbook for test scripts

Patterns for each step of [SKILL.md](../SKILL.md). Examples use `myapp-01` as the instance name; substitute the name your `launch` printed. With exactly one live instance you may omit the name.

## Session lifecycle

```bash
chrome-agent launch --headless        # {"name":"myapp-01","port":9222,"pid":58469,...}
chrome-agent status                   # instances + tabs: ids, urls, titles
chrome-agent stop myapp-01            # shut the browser down when the run ends
chrome-agent status                   # verify the instance is gone
chrome-agent cleanup                  # remove dead instances and stale session dirs
```

`launch` derives the instance name from the working directory and allocates a free port. Everything after `--` passes through to Chrome (`chrome-agent launch -- --lang=de`).

## Finding commands

`help` reads the protocol schema from the running browser, so it always matches the installed Chrome.

```bash
chrome-agent help myapp-01                          # all domains
chrome-agent help myapp-01 Page                     # methods / events / types of one domain
chrome-agent help myapp-01 Page.captureScreenshot   # one signature
```

## Drive the UI

Locate, act, sense again. Coordinates come from the DOM; the click is a press plus a release of trusted input events.

```bash
# Locate: center of the OUTER element (inner nodes can return all zeros)
chrome-agent myapp-01 Runtime.evaluate '{"expression":"(()=>{const el=document.querySelector(\"#submit\");const r=el.getBoundingClientRect();return {x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)};})()","returnByValue":true}'

# Act: real click = mousePressed + mouseReleased at the same coordinates
chrome-agent myapp-01 Input.dispatchMouseEvent '{"type":"mousePressed","x":400,"y":300,"button":"left","clickCount":1}'
chrome-agent myapp-01 Input.dispatchMouseEvent '{"type":"mouseReleased","x":400,"y":300,"button":"left","clickCount":1}'

# Sense again: assert on an independent read
chrome-agent myapp-01 Runtime.evaluate '{"expression":"document.querySelector(\"#result\").textContent","returnByValue":true}'
```

Two kinds of click exist and they are not interchangeable. A synthetic `element.click()` through `Runtime.evaluate` is fabricated in page JS. `Input.dispatchMouseEvent` enters Chrome's native input pipeline, so it reaches what synthetic clicks miss: cross-origin iframes, overlays that intercept capture phase, UIs that check event trust. When a synthetic click silently does nothing, escalate to `Input` events instead of debugging the selector. Plain `click()` is fine on ordinary UIs.

Typing:

```bash
chrome-agent myapp-01 Input.insertText '{"text":"a@b.com"}'                # inserts like an IME
chrome-agent myapp-01 Input.dispatchKeyEvent '{"type":"keyDown","key":"Enter","code":"Enter","windowsVirtualKeyCode":13}'
chrome-agent myapp-01 Input.dispatchKeyEvent '{"type":"keyUp","key":"Enter","code":"Enter","windowsVirtualKeyCode":13}'
```

React-controlled inputs ignore plain value assignment. Set the value through the native setter so React sees the change:

```bash
chrome-agent myapp-01 Runtime.evaluate '{"expression":"(()=>{const el=document.querySelector(\"#email\");const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,\"value\").set;set.call(el,\"a@b.com\");el.dispatchEvent(new Event(\"input\",{bubbles:true}));})()"}'
```

## Waits

Poll readiness before acting on a freshly navigated page:

```bash
chrome-agent myapp-01 Runtime.evaluate '{"expression":"document.readyState","returnByValue":true}'   # until "complete"
```

Event-driven waits: start one `attach` in the background for the whole run, then block per event. Failure events are in the subscription from the start.

```bash
chrome-agent attach myapp-01 +Page.loadEventFired +Page.frameNavigated \
  +Runtime.exceptionThrown +Network.loadingFailed > /tmp/e2e-events.jsonl 2>&1 &

python3 scripts/cdp-wait.py --file /tmp/e2e-events.jsonl \
  --method Page.loadEventFired --timeout 20 --print-offset
```

`cdp-wait.py` (vendored here from the chrome-agent repo's `scripts/`) exits 0 and prints the event JSON the instant a match lands, 1 on timeout. Chain waits by feeding each call's `offset=<n>` stderr output into the next call's `--from-offset`, so already-consumed events do not re-match. Match content, not just method, with `--contains` (repeatable, all-of).

Useful subscriptions:

```bash
+Page.frameNavigated +Page.loadEventFired +Runtime.exceptionThrown +Network.loadingFailed   # default for test runs
+Network.responseReceived                                                                    # what the app calls
+Runtime.consoleAPICalled                                                                    # console output
```

## Asserting

`Runtime.evaluate` with `returnByValue:true` returns `{"result":{"type":...,"value":...}}`; the value is at `result.result.value`. Useful reads:

```js
document.querySelector("#status").textContent
document.querySelector("input#agree").checked
document.querySelectorAll(".cart-item").length
location.pathname
```

The page's in-memory state (framework stores on `window`, data attributes) is more authoritative than the painted DOM; prefer it when both exist.

Screenshots are for layout questions and failure reports, not for reading content. Bytes arrive base64 at the `data` key:

```bash
chrome-agent myapp-01 Page.captureScreenshot '{"format":"png"}' | python3 -c "import sys,json,base64; open('/tmp/shot.png','wb').write(base64.b64decode(json.load(sys.stdin)['data']))"
```

## Talk to the API as the logged-in client

`fetch()` inside the page inherits its session, so same-origin API calls need no credential handling. `awaitPromise` and `returnByValue` are both required; without `awaitPromise` the call returns before the data resolves.

```bash
chrome-agent myapp-01 Runtime.evaluate '{"expression":"fetch(\"/api/orders\").then(r=>r.json())","awaitPromise":true,"returnByValue":true}'
```

To discover endpoints, read the ones the page already called:

```js
performance.getEntriesByType("resource").map(e => e.name)
```

After two or three guessed endpoints 404, attach `+Network.responseReceived` and watch one real request instead of guessing further.

## Tabs

```bash
chrome-agent myapp-01 --url /settings Runtime.evaluate '{...}'   # url substring, most stable
chrome-agent myapp-01 --target 2 Runtime.evaluate '{...}'        # 1-based index
```

Index order sorts by target id, not tab creation order, and opening a tab can renumber the others. Prefer `--url` or a target-id prefix in scripts. A one-shot against multiple tabs with no specifier is an error that lists them.

## Gotchas

- Navigation kills the JS context. A pending `Runtime.evaluate` errors with "context destroyed"; rerun it on the new page.
- One-shot calls cost about 70 ms each (process startup). Tight loops belong in a Python driver, or in the script reading the attach stream.
- One-shots cannot capture Network events; they detach before any fire. Network observation requires `attach` (or the Python API).
- `--target N` with a value under 8 digits is a tab index, anything else a target-id prefix. Prefer the explicit `--target-id` / `--target-index` / `--url` forms in scripts.
- `stop --target` closes a tab, not the browser. Give it an explicit id or index.
- Launched instances keep running until stopped. Stopping is part of the script's teardown, verified by `chrome-agent status`.

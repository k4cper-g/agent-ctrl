"use client"

import * as React from "react"

const TABS = ["snapshot", "act", "waitFor", "get"] as const
type Tab = (typeof TABS)[number]

function Kw({ children }: { children: React.ReactNode }) {
  return <span className="text-violet-300">{children}</span>
}

function Str({ children }: { children: React.ReactNode }) {
  return <span className="text-amber-300">{children}</span>
}

function Fn({ children }: { children: React.ReactNode }) {
  return <span className="text-sky-300">{children}</span>
}

function Cm({ children }: { children: React.ReactNode }) {
  return <span className="text-muted-foreground">{children}</span>
}

function Line({
  n,
  children,
}: {
  n: number
  children: React.ReactNode
}) {
  return (
    <div className="flex">
      <span className="mr-4 inline-block w-4 select-none text-right text-muted-foreground/60">
        {n}
      </span>
      <span className="min-w-0 whitespace-pre">{children}</span>
    </div>
  )
}

const SNIPPETS: Record<Tab, React.ReactNode> = {
  snapshot: (
    <>
      <Line n={1}>
        <Kw>import</Kw> {"{ AgentCtrl }"} <Kw>from</Kw>{" "}
        <Str>&quot;@agent-ctrl/client&quot;</Str>
      </Line>
      <Line n={2}>{" "}</Line>
      <Line n={3}>
        <Kw>const</Kw> ctrl = <Kw>new</Kw> AgentCtrl()
      </Line>
      <Line n={4}>
        <Kw>const</Kw> session = <Kw>await</Kw> ctrl.
        <Fn>openSession</Fn>(<Str>&quot;uia&quot;</Str>)
      </Line>
      <Line n={5}>{" "}</Line>
      <Line n={6}>
        <Cm>// snapshot the current a11y tree</Cm>
      </Line>
      <Line n={7}>
        <Kw>const</Kw> snap = <Kw>await</Kw> ctrl.<Fn>snapshot</Fn>(session)
      </Line>
      <Line n={8}>
        console.<Fn>log</Fn>(snap.refs.entries)
      </Line>
    </>
  ),
  act: (
    <>
      <Line n={1}>
        <Kw>import</Kw> {"{ AgentCtrl }"} <Kw>from</Kw>{" "}
        <Str>&quot;@agent-ctrl/client&quot;</Str>
      </Line>
      <Line n={2}>{" "}</Line>
      <Line n={3}>
        <Kw>const</Kw> ctrl = <Kw>new</Kw> AgentCtrl()
      </Line>
      <Line n={4}>
        <Kw>const</Kw> session = <Kw>await</Kw> ctrl.
        <Fn>openSession</Fn>(<Str>&quot;uia&quot;</Str>)
      </Line>
      <Line n={5}>{" "}</Line>
      <Line n={6}>
        <Cm>// drive elements by ref</Cm>
      </Line>
      <Line n={7}>
        <Kw>await</Kw> ctrl.<Fn>act</Fn>(session, {"{ kind: "}
        <Str>&quot;click&quot;</Str>, ref_id: <Str>&quot;@e0&quot;</Str> {"})"}
      </Line>
      <Line n={8}>
        <Kw>await</Kw> ctrl.<Fn>act</Fn>(session, {"{ kind: "}
        <Str>&quot;fill&quot;</Str>, ref_id: <Str>&quot;@e2&quot;</Str>,
        value: <Str>&quot;hello&quot;</Str> {"})"}
      </Line>
    </>
  ),
  waitFor: (
    <>
      <Line n={1}>
        <Kw>import</Kw> {"{ AgentCtrl }"} <Kw>from</Kw>{" "}
        <Str>&quot;@agent-ctrl/client&quot;</Str>
      </Line>
      <Line n={2}>{" "}</Line>
      <Line n={3}>
        <Kw>const</Kw> ctrl = <Kw>new</Kw> AgentCtrl()
      </Line>
      <Line n={4}>
        <Kw>const</Kw> session = <Kw>await</Kw> ctrl.
        <Fn>openSession</Fn>(<Str>&quot;uia&quot;</Str>)
      </Line>
      <Line n={5}>{" "}</Line>
      <Line n={6}>
        <Cm>// wait until the UI settles</Cm>
      </Line>
      <Line n={7}>
        <Kw>await</Kw> ctrl.<Fn>waitFor</Fn>(session, {"{"}
      </Line>
      <Line n={8}>
        {"  "}predicate: {"{ kind: "}<Str>&quot;stable&quot;</Str>, idle_ms:{" "}
        <Str>250</Str> {"},"} timeout_ms: <Str>5000</Str>,
      </Line>
      <Line n={9}>
        {"}"})
      </Line>
    </>
  ),
  get: (
    <>
      <Line n={1}>
        <Kw>import</Kw> {"{ AgentCtrl }"} <Kw>from</Kw>{" "}
        <Str>&quot;@agent-ctrl/client&quot;</Str>
      </Line>
      <Line n={2}>{" "}</Line>
      <Line n={3}>
        <Kw>const</Kw> ctrl = <Kw>new</Kw> AgentCtrl()
      </Line>
      <Line n={4}>
        <Kw>const</Kw> session = <Kw>await</Kw> ctrl.
        <Fn>openSession</Fn>(<Str>&quot;uia&quot;</Str>)
      </Line>
      <Line n={5}>{" "}</Line>
      <Line n={6}>
        <Cm>// query a property of an element by ref</Cm>
      </Line>
      <Line n={7}>
        <Kw>const</Kw> name = <Kw>await</Kw> ctrl.<Fn>get</Fn>(session,{" "}
        <Str>&quot;name&quot;</Str>, <Str>&quot;@e0&quot;</Str>)
      </Line>
      <Line n={8}>
        console.<Fn>log</Fn>(name?.value)
      </Line>
    </>
  ),
}

export function CodeMock() {
  const [active, setActive] = React.useState<Tab>("snapshot")

  return (
    <div className="flex h-full flex-col border border-border bg-card text-card-foreground">
      <div className="flex items-center justify-between border-b border-border px-4 py-2">
        <div className="flex items-center gap-1.5">
          <span className="size-2.5 rounded-full bg-rose-500/80" />
          <span className="size-2.5 rounded-full bg-amber-400/80" />
          <span className="size-2.5 rounded-full bg-emerald-500/80" />
        </div>
        <span className="font-mono text-xs text-muted-foreground">
          agent.ts
        </span>
        <span className="font-mono text-xs text-muted-foreground">
          run with{" "}
          <span className="rounded-sm border border-border px-1.5 py-0.5 text-foreground">
            agent-ctrl v0.1.1
          </span>
        </span>
      </div>

      <pre className="flex-1 overflow-x-auto px-4 py-5 font-mono text-[13px] leading-6">
        {SNIPPETS[active]}
      </pre>

      <div
        role="tablist"
        aria-label="Code examples"
        className="grid grid-cols-4 divide-x divide-border border-t border-border font-mono text-xs"
      >
        {TABS.map((tab) => (
          <button
            key={tab}
            type="button"
            role="tab"
            aria-selected={tab === active}
            onClick={() => setActive(tab)}
            className={
              tab === active
                ? "px-4 py-3 text-left text-foreground"
                : "px-4 py-3 text-left text-muted-foreground hover:text-foreground"
            }
          >
            {tab}
          </button>
        ))}
      </div>
    </div>
  )
}

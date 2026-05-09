"use client"

import * as React from "react"
import { Check, Copy } from "lucide-react"

const COMMANDS = [
  {
    id: "humans",
    label: "For humans",
    command: "npm install -g @agent-ctrl/cli",
  },
  {
    id: "agents",
    label: "For agents",
    command: "npx skills add k4cper-g/agent-ctrl",
  },
] as const

type Id = (typeof COMMANDS)[number]["id"]

export function InstallTabs() {
  const [active, setActive] = React.useState<Id>("humans")
  const [copied, setCopied] = React.useState(false)
  const command = COMMANDS.find((c) => c.id === active)!.command

  async function onCopy() {
    try {
      await navigator.clipboard.writeText(command)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    } catch {
      // clipboard unavailable; ignore
    }
  }

  return (
    <div className="flex flex-col items-center gap-6">
      <div className="flex items-center gap-4 font-mono text-sm">
        {COMMANDS.map((c, i) => (
          <React.Fragment key={c.id}>
            {i > 0 && (
              <span aria-hidden className="text-muted-foreground/40">
                |
              </span>
            )}
            <button
              type="button"
              onClick={() => {
                setActive(c.id)
                setCopied(false)
              }}
              className={
                c.id === active
                  ? "text-foreground"
                  : "text-muted-foreground hover:text-foreground"
              }
            >
              {c.label}
            </button>
          </React.Fragment>
        ))}
      </div>
      <div className="inline-flex items-center gap-3 border border-border bg-card px-5 py-2.5 font-mono text-sm text-foreground">
        <span className="text-muted-foreground">$</span>
        <span className="select-all">{command}</span>
        <button
          type="button"
          onClick={onCopy}
          aria-label={copied ? "Copied" : "Copy command"}
          className="ml-1 text-muted-foreground hover:text-foreground"
        >
          {copied ? (
            <Check className="size-3.5" />
          ) : (
            <Copy className="size-3.5" />
          )}
        </button>
      </div>
    </div>
  )
}

"use client"

import * as React from "react"
import { Check, Copy } from "lucide-react"

export function InstallCommand({ command }: { command: string }) {
  const [copied, setCopied] = React.useState(false)

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
    <div className="inline-flex h-7 items-center gap-2 border border-border px-3 font-mono text-xs text-foreground">
      <span className="text-muted-foreground">$</span>
      <span className="select-all">{command}</span>
      <button
        type="button"
        onClick={onCopy}
        aria-label={copied ? "Copied" : "Copy install command"}
        className="ml-1 text-muted-foreground hover:text-foreground"
      >
        {copied ? (
          <Check className="size-3" />
        ) : (
          <Copy className="size-3" />
        )}
      </button>
    </div>
  )
}

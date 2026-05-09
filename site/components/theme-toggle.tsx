"use client"

import * as React from "react"
import { Moon, Sun } from "lucide-react"
import { useTheme } from "next-themes"

export function ThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme()
  const [mounted, setMounted] = React.useState(false)

  React.useEffect(() => {
    setMounted(true)
  }, [])

  const isDark = mounted && resolvedTheme === "dark"

  return (
    <button
      type="button"
      onClick={() => setTheme(isDark ? "light" : "dark")}
      aria-label="Toggle theme"
      className="inline-flex size-7 items-center justify-center border border-border text-foreground hover:border-foreground/40"
    >
      {mounted ? (
        isDark ? (
          <Sun className="size-3.5" />
        ) : (
          <Moon className="size-3.5" />
        )
      ) : (
        <span className="size-3.5" />
      )}
    </button>
  )
}

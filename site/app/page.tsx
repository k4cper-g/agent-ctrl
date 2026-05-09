import Image from "next/image"
import Link from "next/link"

import { Button } from "@/components/ui/button"
import { CodeMock } from "@/components/code-mock"
import { InstallCommand } from "@/components/install-command"
import { InstallTabs } from "@/components/install-tabs"
import { ThemeToggle } from "@/components/theme-toggle"

function GithubIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.55 0-.27-.01-1.16-.02-2.1-3.2.7-3.87-1.36-3.87-1.36-.52-1.33-1.27-1.69-1.27-1.69-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.18 1.76 1.18 1.02 1.75 2.68 1.25 3.34.96.1-.74.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.69 0-1.26.45-2.29 1.18-3.1-.12-.29-.51-1.46.11-3.04 0 0 .96-.31 3.15 1.18a10.94 10.94 0 0 1 5.72 0c2.18-1.49 3.14-1.18 3.14-1.18.62 1.58.23 2.75.11 3.04.74.81 1.18 1.84 1.18 3.1 0 4.42-2.69 5.39-5.25 5.68.41.36.78 1.06.78 2.13 0 1.54-.01 2.78-.01 3.16 0 .31.21.67.8.55C20.22 21.39 23.5 17.07 23.5 12 23.5 5.65 18.35.5 12 .5z" />
    </svg>
  )
}

const REPO_OWNER = "k4cper-g"
const REPO_NAME = "agent-ctrl"
const REPO_URL = `https://github.com/${REPO_OWNER}/${REPO_NAME}`

async function getStarCount(): Promise<number | null> {
  try {
    const res = await fetch(
      `https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}`,
      {
        next: { revalidate: 3600 },
        headers: { Accept: "application/vnd.github+json" },
      },
    )
    if (!res.ok) return null
    const data = (await res.json()) as { stargazers_count?: number }
    return typeof data.stargazers_count === "number"
      ? data.stargazers_count
      : null
  } catch {
    return null
  }
}

function formatStars(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 10_000) return `${Math.round(n / 1000)}K`
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}K`
  return String(n)
}

const features = [
  {
    title: "Agent-first.",
    desc: "Compact text output uses fewer tokens than JSON or DOM. Refs let agents target elements deterministically without re-querying.",
  },
  {
    title: "Accessibility-first.",
    desc: "Snapshots the a11y tree, not pixels. Stable across themes, locales, and resolutions where vision models drift.",
  },
  {
    title: "Cross-platform.",
    desc: "UIA on Windows, AX on macOS, AT-SPI on Linux. One schema, native binaries, no runtime.",
  },
]

export default async function Page() {
  const stars = await getStarCount()
  const starsDisplay = stars !== null ? formatStars(stars) : "—"

  const stats = [
    { value: "v0.1.1", label: "Latest release" },
    { value: "Apache 2.0", label: "Open source" },
    { value: "Rust", label: "Native core" },
    { value: "5", label: "Target surfaces" },
  ]

  return (
    <main className="min-h-svh bg-background text-foreground">
      <div className="mx-auto max-w-6xl border border-border">
        <Header stars={starsDisplay} />
        <Stats stats={stats} />
        <Hero />
        <Features />
        <Showcase />
        <Footer />
      </div>
    </main>
  )
}

function Header({ stars }: { stars: string }) {
  return (
    <header className="sticky top-0 z-50 flex items-center justify-between gap-4 border-b border-border bg-background px-6 py-4">
      <Link href="/" className="flex items-center gap-2">
        <Image
          src="/site-logo.png"
          alt="agent-ctrl"
          width={20}
          height={20}
          priority
        />
        <span className="font-mono text-sm tracking-tight">agent-ctrl</span>
      </Link>
      <div className="flex items-center gap-2 font-mono text-xs">
        <div className="hidden md:block">
          <InstallCommand command="npm install -g @agent-ctrl/cli" />
        </div>
        <Link
          href={REPO_URL}
          aria-label={`GitHub repository (${stars} stars)`}
          className="inline-flex h-7 items-center gap-2 border border-border px-2.5 text-foreground hover:border-foreground/40"
        >
          <GithubIcon className="size-3.5" />
          <span>{stars}</span>
        </Link>
        <ThemeToggle />
      </div>
    </header>
  )
}

function Stats({
  stats,
}: {
  stats: ReadonlyArray<{ value: string; label: string }>
}) {
  return (
    <section className="grid grid-cols-2 divide-x divide-y divide-border border-b border-border md:grid-cols-4 md:divide-y-0">
      {stats.map((s) => (
        <div key={s.label} className="px-6 py-8 md:px-8 md:py-10">
          <div className="text-3xl font-medium tracking-tight md:text-4xl">
            {s.value}
          </div>
          <div className="mt-2 font-mono text-[11px] uppercase tracking-wider text-muted-foreground">
            {s.label}
          </div>
        </div>
      ))}
    </section>
  )
}

function Hero() {
  return (
    <section className="border-b border-border px-6 py-16 text-center md:py-24">
      <h1 className="mx-auto max-w-3xl text-3xl font-semibold tracking-tight md:text-5xl">
        OS automation for AI agents.
      </h1>
      <p className="mx-auto mt-6 max-w-xl text-sm leading-relaxed text-muted-foreground md:text-base">
        Drives desktop apps through their accessibility tree. Compact text
        output, ref-based, 100% native Rust.
      </p>
      <div className="mt-10">
        <InstallTabs />
      </div>
    </section>
  )
}

function Features() {
  return (
    <section
      id="features"
      className="grid grid-cols-1 divide-y divide-border border-b border-border md:grid-cols-3 md:divide-x md:divide-y-0"
    >
      {features.map((f) => (
        <div key={f.title} className="px-6 py-10 md:px-8 md:py-12">
          <h3 className="text-base font-semibold">{f.title}</h3>
          <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
            {f.desc}
          </p>
        </div>
      ))}
    </section>
  )
}

function Showcase() {
  return (
    <section
      id="cli"
      className="grid grid-cols-1 divide-y divide-border md:grid-cols-3 md:divide-x md:divide-y-0"
    >
      <div className="flex flex-col gap-10 px-6 py-10 md:px-8 md:py-12">
        <div>
          <h3 className="text-base font-semibold">Fast.</h3>
          <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
            Native Rust binary. Instant command parsing, zero scripting overhead.
            No drivers, no Node, no browser.
          </p>
        </div>
        <div>
          <h3 className="text-base font-semibold">Composable.</h3>
          <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
            Same schema whether you are scripting, inspecting, or feeding an LLM.
            Pipe snapshots and refs through any tool that reads stdin.
          </p>
        </div>
        <div className="mt-auto pt-2">
          <Button asChild>
            <Link href={REPO_URL}>View on GitHub</Link>
          </Button>
        </div>
      </div>
      <div className="md:col-span-2 md:p-6">
        <CodeMock />
      </div>
    </section>
  )
}

function Footer() {
  return (
    <footer className="flex flex-col items-start justify-between gap-4 border-t border-border px-6 py-6 font-mono text-xs text-muted-foreground md:flex-row md:items-center md:px-8">
      <span>agent-ctrl - Apache 2.0</span>
      <Link href={REPO_URL} className="hover:text-foreground">
        github.com/k4cper-g/agent-ctrl
      </Link>
    </footer>
  )
}

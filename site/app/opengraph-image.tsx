import { readFile } from "node:fs/promises"
import { join } from "node:path"
import { ImageResponse } from "next/og"

export const alt = "agent-ctrl - OS automation for AI agents"
export const size = { width: 1200, height: 630 }
export const contentType = "image/png"

export default async function OG() {
  const logoBuf = await readFile(
    join(process.cwd(), "app", "site-logo.png"),
  )
  const logoUrl = `data:image/png;base64,${logoBuf.toString("base64")}`

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          background: "#0a0a0a",
          color: "#fafafa",
          display: "flex",
          flexDirection: "column",
          padding: 80,
          fontFamily: "monospace",
        }}
      >
        <div
          style={{
            fontSize: 28,
            color: "#a3a3a3",
            display: "flex",
            alignItems: "center",
            gap: 12,
          }}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={logoUrl}
            alt="agent-ctrl"
            width={56}
            height={56}
            style={{ objectFit: "contain" }}
          />
          <span>agent-ctrl</span>
        </div>

        <div
          style={{
            marginTop: 56,
            fontSize: 88,
            fontWeight: 600,
            lineHeight: 1.05,
            letterSpacing: "-0.02em",
          }}
        >
          OS automation
        </div>
        <div
          style={{
            fontSize: 88,
            fontWeight: 600,
            lineHeight: 1.05,
            letterSpacing: "-0.02em",
          }}
        >
          for AI agents.
        </div>

        <div
          style={{
            marginTop: 40,
            fontSize: 28,
            color: "#a3a3a3",
            lineHeight: 1.4,
            maxWidth: 900,
            display: "flex",
          }}
        >
          Drives desktop apps through their accessibility tree. Compact,
          ref-based, 100% native Rust.
        </div>

        <div
          style={{
            marginTop: "auto",
            display: "flex",
            alignItems: "center",
            gap: 16,
            border: "1px solid #404040",
            background: "#171717",
            padding: "14px 22px",
            fontSize: 28,
            color: "#fafafa",
            alignSelf: "flex-start",
          }}
        >
          <span style={{ color: "#a3a3a3" }}>$</span>
          npm install -g @agent-ctrl/cli
        </div>
      </div>
    ),
    { ...size },
  )
}

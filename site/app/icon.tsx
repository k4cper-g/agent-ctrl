import { readFile } from "node:fs/promises"
import { join } from "node:path"
import { ImageResponse } from "next/og"

export const size = { width: 32, height: 32 }
export const contentType = "image/png"

export default async function Icon() {
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
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "#0a0a0a",
        }}
      >
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={logoUrl}
          alt="agent-ctrl"
          width={32}
          height={32}
          style={{ objectFit: "contain" }}
        />
      </div>
    ),
    { ...size },
  )
}

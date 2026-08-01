import { fileURLToPath } from "node:url"
import { dirname } from "node:path"

const __dirname = dirname(fileURLToPath(import.meta.url))
const staticExport = process.env.AGENT_CTRL_STATIC_EXPORT === "1"
const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? ""

/** @type {import('next').NextConfig} */
const nextConfig = {
  output: staticExport ? "export" : undefined,
  basePath: staticExport ? basePath : undefined,
  assetPrefix: staticExport ? basePath : undefined,
  trailingSlash: staticExport,
  images: {
    unoptimized: staticExport,
  },
  turbopack: {
    root: __dirname,
  },
}

export default nextConfig

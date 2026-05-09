import type { Metadata } from "next"
import { Geist, Geist_Mono } from "next/font/google"

import "./globals.css"
import { ThemeProvider } from "@/components/theme-provider"
import { cn } from "@/lib/utils";

const geist = Geist({subsets:['latin'],variable:'--font-sans'})

const fontMono = Geist_Mono({
  subsets: ["latin"],
  variable: "--font-mono",
})

const TITLE = "agent-ctrl: OS automation for AI agents"
const DESCRIPTION =
  "Drives desktop apps through their accessibility tree. Compact text output, ref-based, 100% native Rust. Cross-platform CLI for AI agents."

export const metadata: Metadata = {
  metadataBase: new URL("https://agent-ctrl.dev"),
  title: TITLE,
  description: DESCRIPTION,
  applicationName: "agent-ctrl",
  authors: [{ name: "k4cper-g", url: "https://github.com/k4cper-g" }],
  creator: "k4cper-g",
  keywords: [
    "agent-ctrl",
    "AI agents",
    "computer use",
    "desktop automation",
    "accessibility tree",
    "UIA",
    "AX",
    "AT-SPI",
    "CLI",
    "Rust",
  ],
  openGraph: {
    type: "website",
    title: TITLE,
    description: DESCRIPTION,
    url: "/",
    siteName: "agent-ctrl",
    locale: "en_US",
  },
  twitter: {
    card: "summary_large_image",
    title: TITLE,
    description: DESCRIPTION,
  },
  robots: {
    index: true,
    follow: true,
    googleBot: {
      index: true,
      follow: true,
      "max-image-preview": "large",
    },
  },
  alternates: {
    canonical: "/",
  },
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={cn("antialiased", fontMono.variable, "font-sans", geist.variable)}
    >
      <body>
        <ThemeProvider defaultTheme="dark" enableSystem={false}>
          {children}
        </ThemeProvider>
      </body>
    </html>
  )
}

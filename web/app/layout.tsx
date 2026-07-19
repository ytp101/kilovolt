import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Kilovolt (kvlt) ⚡ — The Bankruptcy Shield for AI Stream Engines",
  description: "An ultra-fast, zero-config reverse proxy that intercepts, tokenizes, and terminates AI completions streams the millisecond they cross budget limits.",
  openGraph: {
    title: "Kilovolt (kvlt) ⚡ — The Bankruptcy Shield for AI Stream Engines",
    description: "An ultra-fast, zero-config reverse proxy that intercepts, tokenizes, and terminates AI completions streams the millisecond they cross budget limits.",
    type: "website",
    url: "https://kilovolt.vercel.app/",
    siteName: "Kilovolt",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`${geistSans.variable} ${geistMono.variable} h-full antialiased`}
    >
      <body className="min-h-full flex flex-col">{children}</body>
    </html>
  );
}

import type { Metadata } from 'next'

import '@/app/globals.css'

export const metadata: Metadata = {
  title: 'Ergo · Élő ERP Elemző',
  description: 'Természetes nyelvű üzleti elemzés az élő ERP-adatokon.',
}

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="hu">
      <body>{children}</body>
    </html>
  )
}

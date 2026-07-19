import { useEffect, useState } from 'react'

import { getTheme, setTheme, watchSystemTheme, type Theme } from './lib/theme'

const THEME_ORDER: Theme[] = ['system', 'light', 'dark']

export default function App() {
  const [theme, setThemeState] = useState<Theme>(getTheme)

  useEffect(() => watchSystemTheme(), [])

  const cycleTheme = () => {
    const next = THEME_ORDER[(THEME_ORDER.indexOf(theme) + 1) % THEME_ORDER.length]
    setTheme(next)
    setThemeState(next)
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border bg-surface px-4 py-2">
        <h1 className="text-lg font-medium">CRFty</h1>
        <button
          type="button"
          onClick={cycleTheme}
          className="rounded border border-border bg-elevated px-3 py-1 text-sm transition-colors duration-(--duration-base) hover:bg-overlay"
        >
          Theme: {theme}
        </button>
      </header>
      <main className="flex-1 overflow-auto p-4">
        <p className="text-sm text-muted-foreground">
          UI scaffold — design tokens and formatter ports. Views land once the
          generated bindings exist.
        </p>
      </main>
    </div>
  )
}

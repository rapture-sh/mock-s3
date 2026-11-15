import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import "./index.css"
import { ThemeProvider } from "@/components/theme-provider.tsx"
import { App } from "./App.tsx"

const root = document.getElementById("root")

if (!root) {
  throw new Error("index.html is missing the #root element the console mounts into")
}

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <App />
    </ThemeProvider>
  </StrictMode>,
)

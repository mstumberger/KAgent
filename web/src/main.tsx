import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import '@fontsource-variable/sora'
import './index.css'
import App from './App.tsx'
import { watchForNewBuild } from './freshness'

watchForNewBuild()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)

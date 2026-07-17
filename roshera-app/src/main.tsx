import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '@/components/ui/tooltip'
import { App } from '@/App'
import { installFetchAuth } from '@/lib/auth'
import './index.css'

// Install the auth fetch interceptor before any component mounts, so
// every API request from the app carries the bearer token (when the
// user has logged in) and a 401 surfaces the login prompt. Idempotent.
installFetchAuth()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <TooltipProvider delay={300}>
      <App />
    </TooltipProvider>
  </StrictMode>,
)

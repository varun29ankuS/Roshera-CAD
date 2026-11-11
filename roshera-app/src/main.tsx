import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '@/components/ui/tooltip'
import { App } from '@/App'
import { installFetchAuth } from '@/lib/auth'
import * as THREE from 'three'
import './index.css'

// The kernel is Z-up (a box's height is its Z extent; Agent Eye and the
// drawing views render Z up). The viewport now agrees, so a part stands the
// same way in every view. Set before any camera or control is constructed —
// OrbitControls reads `camera.up` once, at construction.
THREE.Object3D.DEFAULT_UP.set(0, 0, 1)

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

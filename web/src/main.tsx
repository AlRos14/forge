import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { ReactQueryDevtools } from '@tanstack/react-query-devtools'
import { RouterProvider } from '@tanstack/react-router'
import NiceModal from '@ebay/nice-modal-react'
import { Toaster } from 'sonner'
import './index.css'
import { createAppRouter } from './router'
import './lib/i18n'
import { applyThemeFromStorage } from './stores/layout'

applyThemeFromStorage()

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5_000,
      gcTime: 5 * 60_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
})

const router = createAppRouter(queryClient)

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <NiceModal.Provider>
        <RouterProvider router={router} />
        <Toaster richColors position="bottom-right" />
        <ReactQueryDevtools initialIsOpen={false} />
      </NiceModal.Provider>
    </QueryClientProvider>
  </StrictMode>,
)

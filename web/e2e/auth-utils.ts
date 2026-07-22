import { type APIRequestContext, type APIResponse } from '@playwright/test'

const DEFAULT_EMAIL = 'e2e-default@test.forge'
const DEFAULT_PASSWORD = 'Password123!'
const DEFAULT_DISPLAY_NAME = 'E2E Default User'

type AuthResponse = {
  access_token: string
  refresh_token: string
  token_type: string
  expires_in: number
}

type UserResponse = {
  id: string
  email: string
  display_name: string | null
  is_admin: boolean
  created_at: string
}

export type E2EAuth = {
  accessToken: string
  refreshToken: string
  apiToken: string
  user: UserResponse
}

export async function backendAvailable(request: APIRequestContext): Promise<boolean> {
  try {
    const response = await request.get('/healthz', { failOnStatusCode: false })
    return response.ok()
  } catch {
    return false
  }
}

export async function ensureDefaultAuth(request: APIRequestContext): Promise<E2EAuth | null> {
  if (!(await backendAvailable(request))) return null

  const email = process.env.FORGE_E2E_EMAIL ?? DEFAULT_EMAIL
  const password = process.env.FORGE_E2E_PASSWORD ?? DEFAULT_PASSWORD

  const auth = await loginOrRegister(request, email, password)
  const user = await getMe(request, auth.access_token)

  return {
    accessToken: auth.access_token,
    refreshToken: auth.refresh_token,
    apiToken: auth.access_token,
    user,
  }
}

export function fallbackAuth(): E2EAuth {
  const email = process.env.FORGE_E2E_EMAIL ?? DEFAULT_EMAIL
  return {
    accessToken: 'e2e-fallback-access-token',
    refreshToken: 'e2e-fallback-refresh-token',
    apiToken: 'e2e-fallback-access-token',
    user: {
      id: 'e2e-fallback-user',
      email,
      display_name: DEFAULT_DISPLAY_NAME,
      is_admin: true,
      created_at: '2026-01-01T00:00:00Z',
    },
  }
}

export function authStorageValue(auth: E2EAuth): string {
  return JSON.stringify({
    state: {
      accessToken: auth.accessToken,
      refreshToken: auth.refreshToken,
      user: auth.user,
    },
    version: 0,
  })
}

async function loginOrRegister(
  request: APIRequestContext,
  email: string,
  password: string,
): Promise<AuthResponse> {
  const loginResponse = await request.post('/api/v1/auth/login', {
    data: { email, password },
    failOnStatusCode: false,
  })
  if (loginResponse.ok()) return parseJson<AuthResponse>(loginResponse, 'login')

  const registerResponse = await request.post('/api/v1/auth/register', {
    data: {
      email,
      password,
      display_name: DEFAULT_DISPLAY_NAME,
    },
    failOnStatusCode: false,
  })
  if (registerResponse.ok()) return parseJson<AuthResponse>(registerResponse, 'register')

  if (registerResponse.status() === 409) {
    throw new Error(
      `Default e2e user ${email} already exists, but FORGE_E2E_PASSWORD did not authenticate it.`,
    )
  }

  throw new Error(
    `Failed to create default e2e user: ${registerResponse.status()} ${await registerResponse.text()}`,
  )
}

async function getMe(request: APIRequestContext, accessToken: string): Promise<UserResponse> {
  const response = await request.get('/api/v1/auth/me', {
    headers: authHeader(accessToken),
    failOnStatusCode: false,
  })
  await expectOk(response, 'GET /api/v1/auth/me')
  return parseJson<UserResponse>(response, 'me')
}

function authHeader(token: string): Record<string, string> {
  return { authorization: `Bearer ${token}` }
}

async function expectOk(response: APIResponse, label: string): Promise<void> {
  if (response.ok()) return
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`)
}

async function parseJson<T>(response: APIResponse, label: string): Promise<T> {
  try {
    return (await response.json()) as T
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new Error(`Failed to parse ${label} response JSON: ${message}`)
  }
}

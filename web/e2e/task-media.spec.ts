import { readFile, stat } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test, type APIRequestContext, type APIResponse } from './fixtures'

type ProjectResponse = {
  id: string
}

type TaskResponse = {
  id: string
  project_id: string
}

type TaskMediaResponse = {
  id: string
  filename: string
  content_type: string
  byte_size: number
  url: string
}

type PaginatedResponse<T> = {
  items: T[]
  has_more: boolean
}

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(__dirname, '../..')
const imagePath = resolve(repoRoot, 'assets/logo.png')
const videoPath = resolve(repoRoot, 'assets/demo.mp4')

async function api<T>(
  request: APIRequestContext,
  method: 'GET' | 'POST' | 'DELETE',
  path: string,
  data?: unknown,
): Promise<T> {
  const response = await request.fetch(path, {
    method,
    data,
    failOnStatusCode: false,
  })
  await expectOk(response, `${method} ${path}`)
  if (response.status() === 204) return undefined as T
  return (await response.json()) as T
}

async function expectOk(response: APIResponse, label: string) {
  if (response.ok()) return
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`)
}

async function expectDownloadedBytes(
  request: APIRequestContext,
  media: TaskMediaResponse,
  fixtureBytes: Buffer,
) {
  const response = await request.get(media.url, { failOnStatusCode: false })
  await expectOk(response, `GET ${media.url}`)
  expect(response.headers()['content-type']).toContain(media.content_type)
  const body = await response.body()
  expect(body.length).toBe(fixtureBytes.length)
  expect(Buffer.compare(body, fixtureBytes)).toBe(0)
}

test('uploads sample image and demo video as task comment media', async ({ page, request }) => {
  test.setTimeout(120_000)
  const stamp = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  let createdTaskId: string | null = null
  let createdProjectId: string | null = null

  const projectsResponse = await request.get('/api/v1/projects', { failOnStatusCode: false })
  await expectOk(
    projectsResponse,
    'GET /api/v1/projects. Start the Forge API server on localhost:8080 before running this e2e test',
  )

  try {
    const project = await api<ProjectResponse>(request, 'POST', '/api/v1/projects', {
      name: `Media E2E ${stamp}`,
    })
    createdProjectId = project.id

    const task = await api<TaskResponse>(request, 'POST', `/api/v1/projects/${project.id}/tasks`, {
      title: `Media attachment E2E ${stamp}`,
      description: 'Uploads and previews a sample PNG plus demo.mp4.',
    })
    createdTaskId = task.id

    await page.goto(`/tasks/${task.id}/comments`)
    await page.waitForLoadState('domcontentloaded')

    const commentBox = page.getByPlaceholder('Add a comment')
    await expect(commentBox).toBeVisible({ timeout: 15_000 })

    const fileInput = page.locator('input[type="file"]')
    await fileInput.setInputFiles(imagePath)
    await expect(commentBox).toHaveValue(/!\[logo\.png\]\(\/api\/v1\/media\/[^)]+\)/, {
      timeout: 30_000,
    })

    await fileInput.setInputFiles(videoPath)
    await expect(commentBox).toHaveValue(/\[video: demo\.mp4\]\(\/api\/v1\/media\/[^)]+\)/, {
      timeout: 60_000,
    })

    const draft = await commentBox.inputValue()
    const mediaUrls = [...draft.matchAll(/\((\/api\/v1\/media\/[^)]+)\)/g)].map((match) => match[1])
    expect(mediaUrls).toHaveLength(2)
    const [imageUrl, videoUrl] = mediaUrls

    await page.getByRole('button', { name: 'Post' }).click()
    await expect(commentBox).toHaveValue('', { timeout: 15_000 })

    const imagePreview = page.locator(`img[data-media-url="${imageUrl}"]`)
    await expect(imagePreview).toBeVisible({ timeout: 15_000 })
    await expect
      .poll(() =>
        imagePreview.evaluate((node) => {
          const image = node as HTMLImageElement
          return image.complete && image.naturalWidth > 0
        }),
      )
      .toBe(true)

    const videoPreview = page.locator(`video[data-media-url="${videoUrl}"]`)
    await expect(videoPreview).toBeVisible({ timeout: 15_000 })
    await expect(videoPreview).toHaveAttribute('preload', 'metadata')

    const mediaList = await api<PaginatedResponse<TaskMediaResponse>>(
      request,
      'GET',
      `/api/v1/tasks/${task.id}/media`,
    )
    const imageMedia = mediaList.items.find((item) => item.url === imageUrl)
    const videoMedia = mediaList.items.find((item) => item.url === videoUrl)
    expect(imageMedia).toMatchObject({ filename: 'logo.png', content_type: 'image/png' })
    expect(videoMedia).toMatchObject({ filename: 'demo.mp4', content_type: 'video/mp4' })

    const imageBytes = await readFile(imagePath)
    const videoBytes = await readFile(videoPath)
    const imageStats = await stat(imagePath)
    const videoStats = await stat(videoPath)
    expect(imageMedia?.byte_size).toBe(imageStats.size)
    expect(videoMedia?.byte_size).toBe(videoStats.size)
    await expectDownloadedBytes(request, imageMedia!, imageBytes)
    await expectDownloadedBytes(request, videoMedia!, videoBytes)
  } finally {
    if (createdTaskId) {
      await request.delete(`/api/v1/tasks/${createdTaskId}`, { failOnStatusCode: false })
    }
    if (createdProjectId) {
      await request.delete(`/api/v1/projects/${createdProjectId}`, { failOnStatusCode: false })
    }
  }
})

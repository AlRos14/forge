import { mkdir } from 'node:fs/promises'
import path from 'node:path'
import { expect, test } from './fixtures'

type ProjectResponse = {
  id: string
  name: string
}

const proofDir = path.resolve(
  process.cwd(),
  '../target/forge-proof/update-agent-workbench-and-provider-login-2026-08-14',
)

test('Agent Settings exposes truthful provider login choices without starting authorization', async ({
  page,
}) => {
  await page.goto('/agents')

  await expect(page.getByRole('heading', { name: 'Agent Settings', exact: true })).toBeVisible()
  await expect(
    page.getByRole('heading', { name: 'Choose a supported credential method', exact: true }),
  ).toBeVisible()
  await expect(page.getByRole('button', { name: 'Use OpenAI API key', exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Continue in your browser', exact: true })).toBeVisible()
  await expect(page.getByLabel('Identity name', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Model', { exact: true })).toHaveValue('gpt-5.2')
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()

  await expect(
    page.getByRole('button', { name: 'Continue with Google — unavailable', exact: true }),
  ).toBeDisabled()
  await expect(page.getByText(/FORGE_GEMINI_OAUTH_CLIENT_ID/)).toBeVisible()
  await mkdir(proofDir, { recursive: true })
  await page.screenshot({ path: path.join(proofDir, 'live-smoke-provider-desktop.png'), fullPage: true })
})

test('Project Agent Workspace edits typed records and stays contained on mobile', async ({
  page,
  request,
}) => {
  const projectName = `Agent workbench E2E ${Date.now()}`
  const createProject = await request.post('/api/v1/projects', { data: { name: projectName } })
  expect(createProject.ok(), await createProject.text()).toBeTruthy()
  const project = (await createProject.json()) as ProjectResponse

  try {
    await page.goto(`/projects/${project.id}/chat`)
    await expect(
      page.getByRole('heading', { name: `${projectName} Agent Workspace`, exact: true }),
    ).toBeVisible()
    await expect(page.getByLabel('Project editing rail')).toBeVisible()

    const updatedName = `${projectName} updated`
    await page.getByLabel('Project name', { exact: true }).fill(updatedName)
    await page.getByRole('button', { name: 'Save metadata', exact: true }).click()
    await expect(page.getByText(/Project metadata saved at version/)).toBeVisible()

    const taskTitle = `Workbench task ${Date.now()}`
    const taskForm = page
      .locator('form')
      .filter({ has: page.getByRole('button', { name: 'Create Task', exact: true }) })
    await taskForm.getByLabel('Title', { exact: true }).fill(taskTitle)
    await taskForm
      .getByLabel('Description', { exact: true })
      .fill('Created through the typed Project Agent Workspace service.')
    await taskForm.getByRole('button', { name: 'Create Task', exact: true }).click()
    await expect(page.getByText(`Task created: ${taskTitle}`, { exact: true })).toBeVisible()

    await page.setViewportSize({ width: 375, height: 812 })
    const conversationTab = page.getByRole('tab', { name: 'Conversation', exact: true })
    const projectTab = page.getByRole('tab', { name: 'Project', exact: true })
    await expect(conversationTab).toHaveAttribute('aria-selected', 'true')
    await projectTab.click()
    await expect(projectTab).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByLabel('Project editing rail')).toBeVisible()
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBeTruthy()

    await mkdir(proofDir, { recursive: true })
    await page.screenshot({ path: path.join(proofDir, 'live-smoke-mobile.png'), fullPage: true })
  } finally {
    await request.delete(`/api/v1/projects/${project.id}`, { failOnStatusCode: false })
  }
})

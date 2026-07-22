import { spawnSync } from 'node:child_process'
import { rmSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const fixtureUrl = new URL('../src/__typecheck_regression__.ts', import.meta.url)
const fixturePath = fileURLToPath(fixtureUrl)

try {
  writeFileSync(fixturePath, 'const mustBeAString: string = 42\nvoid mustBeAString\n')
  const result = spawnSync('pnpm', ['typecheck', '--pretty', 'false'], {
    cwd: fileURLToPath(new URL('..', import.meta.url)),
    encoding: 'utf8',
  })
  if (result.status === 0) {
    throw new Error('pnpm typecheck unexpectedly accepted an application type error')
  }
  const output = `${result.stdout}\n${result.stderr}`
  if (!output.includes('__typecheck_regression__.ts')) {
    throw new Error(`typecheck failed for an unrelated reason:\n${output}`)
  }
  process.stdout.write('typecheck regression passed: application errors fail the command\n')
} finally {
  rmSync(fixturePath, { force: true })
}

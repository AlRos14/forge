import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

const storageData = new Map<string, string>()
const testLocalStorage: Storage = {
  get length() {
    return storageData.size
  },
  clear() {
    storageData.clear()
  },
  getItem(key: string) {
    return storageData.get(key) ?? null
  },
  key(index: number) {
    return Array.from(storageData.keys())[index] ?? null
  },
  removeItem(key: string) {
    storageData.delete(key)
  },
  setItem(key: string, value: string) {
    storageData.set(key, value)
  },
}

Object.defineProperty(window, 'localStorage', {
  configurable: true,
  value: testLocalStorage,
})

Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  value: testLocalStorage,
})

afterEach(() => {
  cleanup()
  testLocalStorage.clear()
})

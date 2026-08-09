import i18n from 'i18next'
import LanguageDetector from 'i18next-browser-languagedetector'
import { initReactI18next } from 'react-i18next'
import en from '@/locales/en.json'

const productTermKeys = {
  run: { singular: 'terms.run', plural: 'terms.runs' },
  runtime: { singular: 'terms.runtime', plural: 'terms.runtimes' },
  phase: { singular: 'terms.phase', plural: 'terms.phases' },
} as const

export type ProductTerm = keyof typeof productTermKeys

/**
 * Product-facing vocabulary. Backend identifiers stay unchanged; UI copy uses
 * this adapter so the same concept is named consistently across components.
 */
export function productTerm(term: ProductTerm, count = 1): string {
  return i18n.t(productTermKeys[term][count === 1 ? 'singular' : 'plural'])
}

void i18n.use(LanguageDetector).use(initReactI18next).init({
  resources: {
    en: { translation: en },
  },
  fallbackLng: 'en',
  supportedLngs: ['en'],
  interpolation: { escapeValue: false },
})

export default i18n

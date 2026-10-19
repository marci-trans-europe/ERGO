import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const source = fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), '../lib/product-taxonomy.ts'), 'utf8')
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText
const moduleScope = { exports: {} }
new Function('module', 'exports', compiled)(moduleScope, moduleScope.exports)
const { categoryOrder, classifyProduct } = moduleScope.exports

test('MDF-cikkszám és Movink kampány nem termékértékesítés', () => {
  for (const [code, name] of [
    ['MDF', 'AnyDesk MDF'],
    ['ANYDESK_MDF_2026', 'Partner support'],
    ['SEE42', 'SEE FY42 May - Trans-Europe - Movink Product Launch - Marketing'],
  ]) {
    assert.equal(classifyProduct(code, name).category, 'Marketing fund')
    assert.equal(classifyProduct(code, name).kind, 'marketing-fund')
  }
})

test('Autodesk jutalék rebate a CAD-kategóriában', () => {
  const result = classifyProduct('AUTODESK_COSMM_COST', 'S/Autodesk Commission Cost Reseller TDS')
  assert.equal(result.category, 'Mérnöki és CAD')
  assert.equal(result.manufacturer, 'Autodesk')
  assert.equal(result.kind, 'rebate')
})

test('Pantone és Wacom hardver, DeepL AI', () => {
  assert.equal(classifyProduct('GG1504', 'Pantone Plus Pastels and Neons Guide').category, 'Hardver és periféria')
  assert.equal(classifyProduct('W1', 'Wacom Movink').category, 'Hardver és periféria')
  assert.equal(classifyProduct('D1', 'DeepL Pro').category, 'AI és analitika')
})

test('A korábbi általános szoftverkategória megszűnt', () => {
  assert.ok(!categoryOrder.includes('Általános szoftverlicencek és kiegészítők'))
  for (const [code, name] of [
    ['CREDITPACKSMALL', 'AI Credits Starter Pack - 2,000 credits'],
    ['T08M10701J02D', 'TechSmith Camtasia Maintenance Renewal'],
    ['PDFCREATOR', 'PDFCreator Professional single license'],
  ]) {
    assert.ok(categoryOrder.includes(classifyProduct(code, name).category))
  }
})

test('Gyártóvariánsok és név nélküli termékcsaládok egységesítése', () => {
  const cases = [
    ['P1109127', 'Zebra Designer v. 3.0 Pro License', 'Zebra'],
    ['P1109127', 'ZebraDesigner v. 3.0 Pro License', 'Zebra'],
    ['65304579CA01A12', 'All Apps Edition 3 ALL MLP Entrprse Lic Sub New MUE', 'Adobe'],
    ['CREDITPACKSUB365DAYS', 'AI Credits Subscription (1Yr) - 2,500 credits per month', 'Corel'],
    ['CREDITPACKSMALL', 'AI Credits Starter Pack - 2,000 credits', 'Corel'],
    ['CGv1s00C0012e', 'Red Giant Complete Subscription', 'Maxon'],
    ['FM172002LL', 'FileMaker Maintenance', 'Claris'],
    ['', 'Enterprise Architect Ultimate Floating Licence', 'Sparx Systems'],
    ['T08M10701J02D', 'Camtasia 1 year subscription', 'TechSmith'],
    ['ACK24819Z', 'Pro Pen 3 felt pen nibs', 'Wacom'],
    ['INVA2E', 'Able2Extract Professional 21', 'Investintech'],
  ]
  for (const [code, name, expected] of cases) {
    assert.equal(classifyProduct(code, name).manufacturer, expected, name)
  }
})

test('Az előleg és az üres cikk nem kap kitalált gyártót', () => {
  assert.equal(classifyProduct('', 'Előlegszámla_Adobe CC').manufacturer, 'Belső / szolgáltatás')
  assert.equal(classifyProduct('', '(nincs megnevezés)').manufacturer, 'Gyártó nem azonosítható')
})

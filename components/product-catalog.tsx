'use client'

import { useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, LoaderCircle, RefreshCw, Search } from 'lucide-react'

import { loadProductCatalog, loadProductSales } from '@/lib/desktop'
import { categoryOrder, classifyProduct } from '@/lib/product-taxonomy'
import type {
  ProductCatalog,
  ProductCatalogPeriod,
  ProductSale,
  ProductSales,
} from '@/lib/types'

type Product = {
  id: string
  itemNumber: string
  itemName: string
  category: string
  manufacturer: string
  kind: ReturnType<typeof classifyProduct>['kind']
  quantity: number
  netBase: number
  periods: ProductCatalogPeriod[]
}

type ManufacturerGroup = { name: string; products: Product[] }
type CategoryGroup = { name: string; manufacturers: ManufacturerGroup[]; count: number }

const formatNumber = (value: number) =>
  new Intl.NumberFormat('hu-HU', { maximumFractionDigits: 2 }).format(value)

const formatDate = (value: string) => value.replaceAll('.', '-')

const productId = (itemNumber: string, itemName: string) =>
  `${itemNumber}\u001f${itemName}`

const buildProducts = (periods: ProductCatalogPeriod[]): Product[] => {
  const products = new Map<string, Product>()
  for (const period of periods) {
    const id = productId(period.itemNumber, period.itemName)
    let product = products.get(id)
    if (!product) {
      const classification = classifyProduct(period.itemNumber, period.itemName)
      product = {
        id,
        itemNumber: period.itemNumber,
        itemName: period.itemName,
        ...classification,
        quantity: 0,
        netBase: 0,
        periods: [],
      }
      products.set(id, product)
    }
    product.quantity += period.quantity
    product.netBase += period.netBase
    product.periods.push(period)
  }
  return [...products.values()]
}

const groupProducts = (products: Product[]): CategoryGroup[] => {
  const categories = new Map<string, Map<string, Product[]>>()
  for (const product of products) {
    const manufacturers = categories.get(product.category) ?? new Map<string, Product[]>()
    const entries = manufacturers.get(product.manufacturer) ?? []
    entries.push(product)
    manufacturers.set(product.manufacturer, entries)
    categories.set(product.category, manufacturers)
  }
  return [...categories.entries()]
    .map(([name, manufacturers]) => ({
      name,
      manufacturers: [...manufacturers.entries()]
        .map(([manufacturer, entries]) => ({
          name: manufacturer,
          products: entries.sort((a, b) =>
            Math.abs(b.netBase) - Math.abs(a.netBase) ||
            a.itemName.localeCompare(b.itemName, 'hu'),
          ),
        }))
        .sort((a, b) => a.name.localeCompare(b.name, 'hu')),
      count: [...manufacturers.values()].reduce((sum, entries) => sum + entries.length, 0),
    }))
    .sort((a, b) => categoryOrder.indexOf(a.name as typeof categoryOrder[number]) -
      categoryOrder.indexOf(b.name as typeof categoryOrder[number]))
}

const kindLabel: Record<Product['kind'], string> = {
  product: 'Termék',
  'marketing-fund': 'Marketing fund · nem termékértékesítés',
  rebate: 'Rebate · nem termékértékesítés',
  service: 'Szolgáltatás · nem termékértékesítés',
}

export const ProductCatalogView = () => {
  const [catalog, setCatalog] = useState<ProductCatalog | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const [openCategories, setOpenCategories] = useState<string[]>([])
  const [openManufacturers, setOpenManufacturers] = useState<string[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [sales, setSales] = useState<ProductSales | null>(null)
  const [salesLoading, setSalesLoading] = useState(false)
  const [salesError, setSalesError] = useState<string | null>(null)
  const [trendMetric, setTrendMetric] = useState<'quantity' | 'netBase'>('quantity')
  const salesRequest = useRef(0)

  const reload = async () => {
    setLoading(true)
    setError(null)
    try {
      const result = await loadProductCatalog()
      setCatalog(result)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    let active = true
    loadProductCatalog()
      .then((result) => {
        if (active) setCatalog(result)
      })
      .catch((cause) => {
        if (active) setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => { active = false }
  }, [])

  const allProducts = useMemo(
    () => buildProducts(catalog?.periods ?? []),
    [catalog],
  )
  const filteredProducts = useMemo(() => {
    const query = search.toLocaleLowerCase('hu-HU').trim()
    if (!query) return allProducts
    return allProducts.filter((product) =>
      `${product.itemNumber} ${product.itemName} ${product.manufacturer} ${product.category}`
        .toLocaleLowerCase('hu-HU').includes(query),
    )
  }, [allProducts, search])
  const categories = useMemo(() => groupProducts(filteredProducts), [filteredProducts])
  const selected = allProducts.find((product) => product.id === selectedId) ?? null
  const actualProducts = allProducts.filter((product) => product.kind === 'product').length

  const toggle = (name: string, current: string[], setter: (next: string[]) => void) => {
    setter(current.includes(name) ? current.filter((item) => item !== name) : [...current, name])
  }

  const selectProduct = async (product: Product) => {
    const request = ++salesRequest.current
    setSelectedId(product.id)
    setSales(null)
    setSalesError(null)
    setSalesLoading(false)
    setTrendMetric('quantity')
    if (!product.itemNumber) {
      setSalesError('Ehhez a sorhoz nincs cikkszám; a vevői tételek nem azonosíthatók biztonságosan.')
      return
    }
    setSalesLoading(true)
    try {
      const result = await loadProductSales(product.itemNumber, product.itemName)
      if (request === salesRequest.current) setSales(result)
    } catch (cause) {
      if (request === salesRequest.current) {
        setSalesError(cause instanceof Error ? cause.message : String(cause))
      }
    } finally {
      if (request === salesRequest.current) setSalesLoading(false)
    }
  }

  const quarters = [1, 2, 3, 4].map((quarter) => ({
    quarter,
    quantity: selected?.periods
      .filter((period) => period.year === catalog?.currentYear && period.quarter === quarter)
      .reduce((sum, period) => sum + period.quantity, 0) ?? 0,
    netBase: selected?.periods
      .filter((period) => period.year === catalog?.currentYear && period.quarter === quarter)
      .reduce((sum, period) => sum + period.netBase, 0) ?? 0,
  }))
  const maxTrend = Math.max(1, ...quarters.map((item) => Math.abs(item[trendMetric])))

  return (
    <div className="catalog-view">
      <div className="catalog-toolbar">
        <div>
          <p className="header-kicker">ÉLŐ ERP-KATALÓGUS</p>
          <h1>Termékkategóriák</h1>
          <p>Gyártók és cikkek · FY {catalog ? `${catalog.currentYear - 2}–${catalog.currentYear}` : '…'} · AI-token nélkül</p>
        </div>
        <button disabled={loading} onClick={() => void reload()} type="button">
          <RefreshCw aria-hidden="true" size={16} /> Frissítés
        </button>
      </div>

      {error ? <p className="catalog-error" role="alert">{error}</p> : null}
      {loading ? <p className="catalog-loading"><LoaderCircle className="spin" size={18} /> Katalógus betöltése…</p> : null}

      {catalog ? (
        <>
          <div className="catalog-stats">
            <span><strong>{formatNumber(actualProducts)}</strong> termék</span>
            <span><strong>{formatNumber(allProducts.length - actualProducts)}</strong> nem termékjellegű tétel</span>
            <span><strong>{formatNumber(categories.length)}</strong> kategória</span>
          </div>
          <div className="catalog-grid">
            <aside className="catalog-tree" aria-label="Termékkategóriák, gyártók és cikkek">
              <label className="catalog-search">
                <Search aria-hidden="true" size={16} />
                <input
                  aria-label="Keresés a katalógusban"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Gyártó, cikkszám vagy cikk…"
                  type="search"
                  value={search}
                />
              </label>
              {categories.length ? categories.map((category) => {
                const categoryOpen = search.trim() || openCategories.includes(category.name)
                return <div className="catalog-category" key={category.name}>
                  <button
                    aria-expanded={Boolean(categoryOpen)}
                    className="catalog-category-button"
                    onClick={() => toggle(category.name, openCategories, setOpenCategories)}
                    type="button"
                  >
                    <ChevronDown className={categoryOpen ? 'is-open' : ''} size={15} />
                    <span>{category.name}</span><small>{category.count}</small>
                  </button>
                  {categoryOpen ? category.manufacturers.map((manufacturer) => {
                    const key = `${category.name}\u001f${manufacturer.name}`
                    const manufacturerOpen = search.trim() || openManufacturers.includes(key)
                    return <div className="catalog-manufacturer" key={key}>
                      <button
                        aria-expanded={Boolean(manufacturerOpen)}
                        className="catalog-manufacturer-button"
                        onClick={() => toggle(key, openManufacturers, setOpenManufacturers)}
                        type="button"
                      >
                        <ChevronDown className={manufacturerOpen ? 'is-open' : ''} size={14} />
                        <span>{manufacturer.name}</span><small>{manufacturer.products.length}</small>
                      </button>
                      {manufacturerOpen ? manufacturer.products.map((product) =>
                        <button
                          aria-current={selectedId === product.id ? 'true' : undefined}
                          className={`catalog-product-button ${selectedId === product.id ? 'active' : ''}`}
                          key={product.id}
                          onClick={() => void selectProduct(product)}
                          title={product.itemName}
                          type="button"
                        >
                          {product.itemName}
                        </button>,
                      ) : null}
                    </div>
                  }) : null}
                </div>
              }) : <p className="catalog-empty">Nincs találat.</p>}
            </aside>

            <section className="catalog-detail" aria-live="polite">
              {selected ? <>
                <div className="catalog-detail-heading">
                  <div>
                    <p className="header-kicker">{selected.category} / {selected.manufacturer}</p>
                    <h2>{selected.itemName}</h2>
                    <p className="catalog-code">Cikkszám: {selected.itemNumber || '–'} · {kindLabel[selected.kind]}</p>
                  </div>
                </div>
                <div className="catalog-product-stats">
                  <span>3 FY nettó mennyiség <strong>{formatNumber(selected.quantity)}</strong></span>
                  <span>3 FY nettó alapdevizás érték <strong>{formatNumber(selected.netBase)}</strong></span>
                </div>
                <div className="catalog-chart-heading">
                  <h3>{catalog.currentYear}. évi negyedéves trend</h3>
                  <div role="group" aria-label="Diagram mérőszáma">
                    <button aria-pressed={trendMetric === 'quantity'} onClick={() => setTrendMetric('quantity')} type="button">Mennyiség</button>
                    <button aria-pressed={trendMetric === 'netBase'} onClick={() => setTrendMetric('netBase')} type="button">Nettó érték</button>
                  </div>
                </div>
                <div className="catalog-chart" role="img" aria-label={`${catalog.currentYear}. évi negyedéves ${trendMetric === 'quantity' ? 'mennyiség' : 'nettó érték'} oszlopdiagram`}>
                  {quarters.map((item) => <div className="catalog-quarter" key={item.quarter}>
                    <span>{formatNumber(item[trendMetric])}</span>
                    <div className="catalog-column-track">
                      <div
                        className={item[trendMetric] < 0 ? 'negative' : ''}
                        style={{ height: `${Math.abs(item[trendMetric]) / maxTrend * 100}%` }}
                        title={`${item.quarter}. negyedév: ${formatNumber(item[trendMetric])}`}
                      />
                    </div>
                    <strong>Q{item.quarter}</strong>
                  </div>)}
                </div>
                <h3 className="catalog-sales-title">{selected.kind === 'product' ? 'Kinek és mikor adtuk el?' : 'Mely számlákon szerepelt?'}</h3>
                <p className="catalog-sales-note">{selected.kind !== 'product' ? 'Ez nem termékértékesítés; az alábbiak a kapcsolódó könyvelt cikksorok. ' : ''}Az elmúlt 3 FY számlasorai; a negatív érték jóváírás. A viszonteladó a számlázott vevő, a végfelhasználó ettől eltérhet.</p>
                {salesLoading ? <p className="catalog-loading"><LoaderCircle className="spin" size={16} /> Számlák betöltése…</p> : null}
                {salesError ? <p className="catalog-error" role="alert">{salesError}</p> : null}
                {sales ? <SalesTable sales={sales.rows} truncated={sales.truncated} /> : null}
              </> : <div className="catalog-detail-empty">
                <h2>Válassz egy cikket</h2>
                <p>Nyisd meg a kategóriát, utána a gyártót. A cikkre kattintva megjelenik az idei negyedéves trend, valamint a számlázott viszonteladók és végfelhasználók listája.</p>
              </div>}
            </section>
          </div>
        </>
      ) : null}
    </div>
  )
}

const SalesTable = ({ sales, truncated }: { sales: ProductSale[]; truncated: boolean }) => (
  <>
    {truncated ? <p className="catalog-warning">Csak a legutóbbi 300 számlasor látható; a lista nem teljes.</p> : null}
    {sales.length ? <div className="catalog-sales-wrap"><table>
      <thead><tr><th>Dátum</th><th>Számla</th><th>Viszonteladó / számlázott vevő</th><th>Végfelhasználó</th><th>Mennyiség</th><th>Nettó érték</th></tr></thead>
      <tbody>{sales.map((sale, index) => <tr key={`${sale.invoiceNumber}-${index}`}>
        <td>{formatDate(sale.invoiceDate)}</td>
        <td>{sale.invoiceNumber}</td>
        <td>{sale.reseller}</td>
        <td>{sale.endCustomer}</td>
        <td className="catalog-number">{formatNumber(sale.quantity)}</td>
        <td className="catalog-number">{formatNumber(sale.netBase)}</td>
      </tr>)}</tbody>
    </table></div> : <p className="catalog-empty">Ehhez a cikkhez a vizsgált időszakban nincs megjeleníthető számlasor.</p>}
  </>
)

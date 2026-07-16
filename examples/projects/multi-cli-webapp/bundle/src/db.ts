// Stand-in data layer. A real app would talk to Postgres here; the sample
// keeps an in-memory list so the repo runs with no services attached.
export interface Product {
  id: string;
  name: string;
  cents: number;
}

const PRODUCTS: Product[] = [
  { id: "11111111-1111-4111-8111-111111111111", name: "Cotton tee", cents: 1800 },
  { id: "22222222-2222-4222-8222-222222222222", name: "Canvas tote", cents: 2400 },
];

export async function listProducts(): Promise<Product[]> {
  return PRODUCTS;
}

export async function getProduct(id: string): Promise<Product | undefined> {
  return PRODUCTS.find((p) => p.id === id);
}

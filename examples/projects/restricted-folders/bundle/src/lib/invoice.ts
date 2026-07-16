// Pure billing math — safe, ordinary application code.
export interface LineItem { description: string; cents: number; }
export interface InvoiceInput { customerId: string; lineItems: LineItem[]; }
export interface Invoice { customerId: string; total: number; }

export function computeInvoice(input: InvoiceInput): Invoice {
  const total = input.lineItems.reduce((sum, li) => sum + li.cents, 0);
  return { customerId: input.customerId, total };
}

// acme-billing — service entrypoint (agents may freely read/edit this).
import { computeInvoice } from "./lib/invoice";

export function main(): void {
  const invoice = computeInvoice({ customerId: "cus_demo", lineItems: [] });
  console.log(`invoice total: ${invoice.total}`);
}

main();

// HTTP routes. Every request body is validated at the boundary with zod,
// and errors go out in one shared envelope — see the house API conventions.
import type { FastifyInstance } from "fastify";
import { z } from "zod";
import { listProducts, getProduct } from "./db.js";

const idParam = z.object({ id: z.string().uuid() });

export function registerRoutes(app: FastifyInstance): void {
  app.get("/products", async () => ({ data: await listProducts() }));

  app.get("/products/:id", async (req, reply) => {
    const parsed = idParam.safeParse(req.params);
    if (!parsed.success) {
      return reply.code(400).send({ error: { code: "bad_request", message: "invalid id" } });
    }
    const product = await getProduct(parsed.data.id);
    if (!product) {
      return reply.code(404).send({ error: { code: "not_found", message: "no such product" } });
    }
    return { data: product };
  });
}

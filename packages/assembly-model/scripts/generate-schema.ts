import { writeFileSync, mkdirSync } from "node:fs";
import { z } from "zod";
import { ApplianceClassSchema, ApplicationAssemblySchema } from "../src/schema";

mkdirSync(new URL("../schema/", import.meta.url), { recursive: true });
const write = (name: string, schema: z.ZodType) =>
  writeFileSync(new URL(`../schema/${name}.schema.json`, import.meta.url), JSON.stringify(z.toJSONSchema(schema), null, 2) + "\n");
write("application-assembly", ApplicationAssemblySchema);
write("appliance-class", ApplianceClassSchema);
console.log("schemas written");

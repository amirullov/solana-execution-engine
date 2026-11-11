import { z } from 'zod';

export const executionModeSchema = z.literal('disabled');

export const clusterSchema = z.enum(['localnet', 'devnet', 'mainnet-beta']);

export const executionIntentSchema = z
  .object({
    idempotencyKey: z.string().min(8).max(128),
    cluster: clusterSchema.default('devnet'),
    inputMint: z.string().min(32).max(44),
    outputMint: z.string().min(32).max(44),
    amountAtomic: z.string().regex(/^[1-9][0-9]*$/),
    maxSlippageBps: z.number().int().min(0).max(10_000),
  })
  .strict()
  .refine((intent) => intent.inputMint !== intent.outputMint, {
    message: 'inputMint and outputMint must differ',
    path: ['outputMint'],
  });

export type ExecutionIntent = z.infer<typeof executionIntentSchema>;

export const intentAcceptedSchema = z.object({
  id: z.uuid(),
  status: z.literal('simulation_only'),
  executionMode: executionModeSchema,
});

export type IntentAccepted = z.infer<typeof intentAcceptedSchema>;

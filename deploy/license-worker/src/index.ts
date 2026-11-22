export interface Env {
  SIGNING_KEY_B64: string;       // exact contents of linkguard-private.key
  STRIPE_WEBHOOK_SECRET: string; // whsec_... from Stripe Dashboard
  RESEND_API_KEY: string;        // optional: for automated email delivery
  FROM_EMAIL: string;            // e.g. "LinkGuard <licenses@yourdomain.com>"
}

const PKCS8_ED25519_PREFIX = new Uint8Array([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
  0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
]);

function toBase64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(s: string): Uint8Array {
  const b64 = s.trim().replace(/-/g, "+").replace(/_/g, "/") +
    "=".repeat((4 - (s.trim().length % 4)) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function timingSafeEqualHex(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

async function verifyStripeSignature(
  rawBody: string,
  sigHeader: string | null,
  secret: string,
): Promise<boolean> {
  if (!sigHeader) return false;
  const parts = Object.fromEntries(
    sigHeader.split(",").map((kv) => kv.split("=") as [string, string]),
  );
  const timestamp = parts["t"];
  const v1 = parts["v1"];
  if (!timestamp || !v1) return false;

  const age = Math.abs(Math.floor(Date.now() / 1000) - Number(timestamp));
  if (!Number.isFinite(age) || age > 300) return false;

  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    new TextEncoder().encode(`${timestamp}.${rawBody}`),
  );
  const expectedHex = [...new Uint8Array(mac)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");

  return timingSafeEqualHex(expectedHex, v1);
}

async function mintLicenseToken(
  seedB64: string,
  tier: "pro" | "team",
  holder: string,
  licenseId: string,
  days: number | null,
): Promise<string> {
  const seed = fromBase64Url(seedB64);
  if (seed.length !== 32) throw new Error("SIGNING_KEY_B64 must be 32 bytes");

  const pkcs8 = new Uint8Array(PKCS8_ED25519_PREFIX.length + 32);
  pkcs8.set(PKCS8_ED25519_PREFIX, 0);
  pkcs8.set(seed, PKCS8_ED25519_PREFIX.length);

  const privateKey = await crypto.subtle.importKey(
    "pkcs8",
    pkcs8,
    { name: "Ed25519" },
    false,
    ["sign"],
  );

  const now = Math.floor(Date.now() / 1000);
  const claims = {
    tier,
    holder,
    issued_unix: now,
    expires_unix: days ? now + days * 86_400 : null,
    license_id: licenseId,
  };

  const payloadB64 = toBase64Url(new TextEncoder().encode(JSON.stringify(claims)));
  const msg = `LG1.${payloadB64}`;
  const sig = new Uint8Array(
    await crypto.subtle.sign("Ed25519", privateKey, new TextEncoder().encode(msg)),
  );
  return `${msg}.${toBase64Url(sig)}`;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method !== "POST") {
      return new Response("Method Not Allowed", { status: 405 });
    }

    const rawBody = await request.text();
    const ok = await verifyStripeSignature(
      rawBody,
      request.headers.get("stripe-signature"),
      env.STRIPE_WEBHOOK_SECRET,
    );
    if (!ok) return new Response("Invalid signature", { status: 400 });

    const event = JSON.parse(rawBody);
    if (event.type !== "checkout.session.completed") {
      return new Response("Ignored event", { status: 200 });
    }

    const session = event.data.object;
    const email: string =
      session.customer_details?.email || session.customer_email || "customer";
    const tier = session.metadata?.tier === "team" ? "team" : "pro";
    const days = session.metadata?.perpetual === "true" ? null : 365;

    const token = await mintLicenseToken(
      env.SIGNING_KEY_B64,
      tier,
      email,
      session.id,
      days,
    );

    if (env.RESEND_API_KEY) {
      await fetch("https://api.resend.com/emails", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${env.RESEND_API_KEY}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          from: env.FROM_EMAIL,
          to: [email],
          subject: `Your LinkGuard ${tier.toUpperCase()} Offline License`,
          text:
            `Thanks for purchasing LinkGuard ${tier.toUpperCase()}!\n\n` +
            `Save this token to linkguard.lic or set LINKGUARD_LICENSE:\n\n${token}\n\n` +
            `Verify offline anytime with: linkguard license\n`,
        }),
      });
    }

    return new Response(JSON.stringify({ issued: true, license_id: session.id }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  },
};
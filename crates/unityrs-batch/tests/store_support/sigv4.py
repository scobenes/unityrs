"""Fixed test reference using only Python stdlib, independent of Rust signers."""
import hashlib
import hmac


def mac(key, value):
    return hmac.new(key, value, hashlib.sha256).digest()


payload = hashlib.sha256(b"hello").hexdigest()
canonical = (
    "PUT\n/bucket/a%25%23%20%E9%9B%AA\n"
    "list-type=2&max-keys=1&prefix=a%20b\n"
    "host:example.test:9000\n"
    f"x-amz-content-sha256:{payload}\n"
    "x-amz-date:20260101T123456Z\n"
    "x-amz-security-token:session-sentinel\n\n"
    "host;x-amz-content-sha256;x-amz-date;x-amz-security-token\n"
    f"{payload}"
)
scope = "20260101/us-east-1/s3/aws4_request"
to_sign = (
    "AWS4-HMAC-SHA256\n20260101T123456Z\n"
    f"{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"
)
key = mac(b"AWS4secret", b"20260101")
for part in [b"us-east-1", b"s3", b"aws4_request"]:
    key = mac(key, part)
print(mac(b"\x0b" * 20, b"Hi There").hex())
print(mac(key, to_sign.encode()).hex())
print(payload)

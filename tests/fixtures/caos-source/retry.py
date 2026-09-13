"""Retry requests after temporary network failures."""
import time

def retry(operation, attempts=3):
    """Retry with exponential backoff; preserve the final error."""
    for attempt in range(attempts):
        try:
            return operation()
        except ConnectionError:
            if attempt + 1 == attempts:
                raise
            time.sleep(2 ** attempt)

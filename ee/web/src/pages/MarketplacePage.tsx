import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Star, Package } from "lucide-react";
import { listMarketplace } from "@/lib/api";
import type { MarketplaceListing } from "@/types/api";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Badge } from "@/components/ui/Badge";
import { PageSkeleton } from "@/components/ui/Skeleton";

/** A rating rendered as filled stars, so a number does not have to be read as a quality. */
function Stars({ value }: { value: number }) {
  return (
    <span className="inline-flex items-center gap-0.5" aria-label={`${value.toFixed(1)} out of 5`}>
      {[1, 2, 3, 4, 5].map((n) => (
        <Star
          key={n}
          aria-hidden
          className={
            n <= Math.round(value)
              ? "h-3.5 w-3.5 fill-amber-400 text-amber-400"
              : "h-3.5 w-3.5 text-muted-foreground/40"
          }
        />
      ))}
    </span>
  );
}

/// Matches the server's own default page size, so "a full page came back" means "there may be
/// more" rather than depending on a number the two sides could disagree about silently.
const PAGE_SIZE = 50;

export function MarketplacePage() {
  const [listings, setListings] = useState<MarketplaceListing[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const first = await listMarketplace({ limit: PAGE_SIZE });
      setListings(first);
      setHasMore(first.length === PAGE_SIZE);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the marketplace");
    } finally {
      setLoading(false);
    }
  }

  async function loadMore() {
    setLoadingMore(true);
    setError(null);
    try {
      const next = await listMarketplace({ offset: listings.length, limit: PAGE_SIZE });
      setListings((prev) => [...prev, ...next]);
      setHasMore(next.length === PAGE_SIZE);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load more templates");
    } finally {
      setLoadingMore(false);
    }
  }

  useEffect(() => {
    load();
  }, []);

  if (loading) return <PageSkeleton />;

  return (
    <div className="space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Marketplace</h1>
        <p className="text-sm text-muted-foreground">
          Schema templates other organizations have published. Forking copies one into your own
          library, where it stays private until you publish it yourself.
        </p>
      </div>

      {error && (
        <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      )}

      {!error && listings.length === 0 && (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            Nothing has been published yet. A template appears here once its owner marks it
            community-visible and publishes a version.
          </CardContent>
        </Card>
      )}

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
        {listings.map((listing) => (
          <Card key={listing.template_id} className="flex flex-col">
            <CardHeader>
              <div className="flex items-start justify-between gap-2">
                <CardTitle className="text-lg">{listing.name}</CardTitle>
                {listing.latest_stable_version !== null && (
                  <Badge variant="secondary">v{listing.latest_stable_version}</Badge>
                )}
              </div>
              {listing.description && <CardDescription>{listing.description}</CardDescription>}
            </CardHeader>
            <CardContent className="flex flex-1 flex-col justify-between gap-3">
              <div className="flex flex-wrap gap-1">
                {listing.tags.map((tag) => (
                  <Badge key={tag} variant="outline">
                    {tag}
                  </Badge>
                ))}
              </div>
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                {listing.average_rating === null ? (
                  <span>No reviews yet</span>
                ) : (
                  <span className="inline-flex items-center gap-1.5">
                    <Stars value={listing.average_rating} />
                    {listing.average_rating.toFixed(1)} ({listing.review_count})
                  </span>
                )}
                {listing.author && <span>by {listing.author}</span>}
              </div>
              <Link to={`/marketplace/${encodeURIComponent(listing.template_id)}`}>
                <Button size="sm" variant="secondary">
                  <Package className="mr-1 h-3.5 w-3.5" />
                  Details
                </Button>
              </Link>
            </CardContent>
          </Card>
        ))}
      </div>

      {hasMore && (
        <div className="flex justify-center">
          <Button variant="secondary" onClick={loadMore} disabled={loadingMore}>
            {loadingMore ? "Loading…" : "Load more"}
          </Button>
        </div>
      )}
    </div>
  );
}

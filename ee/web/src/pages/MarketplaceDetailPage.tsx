import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, GitFork, Star } from "lucide-react";
import {
  listMarketplace,
  listTemplateVersions,
  listTemplateReviews,
  forkMarketplaceTemplate,
  submitTemplateReview,
} from "@/lib/api";
import type { MarketplaceListing, TemplateVersion, TemplateReview } from "@/types/api";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Badge } from "@/components/ui/Badge";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { SchemaStructureCard } from "@/components/schema/SchemaStructureCard";
import { SchemaDefinitionTables } from "@/components/schema/SchemaDefinitionTables";
import { formatDate, formatDateTime } from "@/lib/format";

function Stars({ value }: { value: number }) {
  return (
    <span className="inline-flex items-center gap-0.5" aria-label={`${value} out of 5`}>
      {[1, 2, 3, 4, 5].map((n) => (
        <Star
          key={n}
          className={
            n <= value
              ? "h-3.5 w-3.5 fill-warning text-warning"
              : "h-3.5 w-3.5 text-muted-foreground"
          }
        />
      ))}
    </span>
  );
}

/// A marketplace template's own page, at `/marketplace/:id`.
///
/// The listing grid used to open a dialog, which meant a template could not be linked to,
/// reloaded, or opened in a tab — and, more to the point, could not show the schema it contains.
/// Forking is a decision about structure, so the page shows the same structure graph and
/// entity/relation tables `/schemas/:id` does, from the definition the versions endpoint already
/// returns.
export function MarketplaceDetailPage() {
  const { templateId } = useParams<{ templateId: string }>();

  const [listing, setListing] = useState<MarketplaceListing | null>(null);
  const [versions, setVersions] = useState<TemplateVersion[]>([]);
  const [reviews, setReviews] = useState<TemplateReview[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [rating, setRating] = useState(5);
  const [comment, setComment] = useState("");
  /// Which version's definition is shown. Defaults to the newest, so the page opens on what a
  /// plain fork would give you.
  const [shownVersion, setShownVersion] = useState<number | null>(null);

  const load = useCallback(async () => {
    if (!templateId) return;
    setLoading(true);
    setError(null);
    try {
      // The listing carries the name, author and review aggregates; there is no single-listing
      // endpoint, so it is picked out of the collection.
      const [all, v, r] = await Promise.all([
        listMarketplace(),
        listTemplateVersions(templateId),
        listTemplateReviews(templateId),
      ]);
      const found = all.find((l) => l.template_id === templateId) ?? null;
      setListing(found);
      setVersions(v);
      setReviews(r);
      setShownVersion((prev) => prev ?? (v.length > 0 ? v[0].version : null));
      if (!found) setError("This template is no longer listed in the marketplace.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load this template");
    } finally {
      setLoading(false);
    }
  }, [templateId]);

  useEffect(() => {
    load();
  }, [load]);

  async function handleFork(version?: number) {
    if (!templateId) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await forkMarketplaceTemplate(templateId, version);
      setNotice("Forked into your template library. It is private until you publish it.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to fork");
    } finally {
      setBusy(false);
    }
  }

  async function handleReview() {
    if (!templateId) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await submitTemplateReview(templateId, rating, comment.trim() || null);
      setComment("");
      await load();
      setNotice("Review saved.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save the review");
    } finally {
      setBusy(false);
    }
  }

  if (loading) return <PageSkeleton />;

  const shown = versions.find((v) => v.version === shownVersion) ?? versions[0] ?? null;
  const definition = shown?.definition ?? null;

  return (
    <div className="space-y-6 p-6">
      <Link
        to="/marketplace"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Back to Marketplace
      </Link>

      {error && (
        <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      )}
      {notice && (
        <div className="rounded-md border border-border bg-secondary p-3 text-sm text-secondary-foreground">
          {notice}
        </div>
      )}

      <Card>
        <CardHeader>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-3">
                <CardTitle>{listing?.name ?? "Template"}</CardTitle>
                {shown && (
                  <Badge variant={shown.status === "stable" ? "default" : "secondary"}>
                    v{shown.version} {shown.status}
                  </Badge>
                )}
              </div>
              {listing?.description && <CardDescription>{listing.description}</CardDescription>}
            </div>
            <Button
              disabled={busy || versions.length === 0}
              onClick={() => handleFork(shown?.version)}
            >
              <GitFork className="mr-1 h-4 w-4" />
              Fork this version
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Author</dt>
              <dd className="text-sm">{listing?.author ?? "—"}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Latest stable</dt>
              <dd className="text-sm">
                {listing?.latest_stable_version ? `v${listing.latest_stable_version}` : "—"}
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Rating</dt>
              <dd className="text-sm">
                {listing?.average_rating === null || listing?.average_rating === undefined ? (
                  "No reviews yet"
                ) : (
                  <span className="inline-flex items-center gap-1.5">
                    <Stars value={Math.round(listing.average_rating)} />
                    <span className="text-muted-foreground">({listing.review_count})</span>
                  </span>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Published</dt>
              <dd className="text-sm">{shown ? formatDateTime(shown.created_at) : "—"}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>

      {/* The reason this is a page rather than a dialog: forking is a decision about structure,
          and the structure was the one thing the dialog could not show. */}
      <SchemaStructureCard
        definition={definition}
        description="Entity types and their relations in the version shown above."
      />

      {definition && <SchemaDefinitionTables definition={definition} />}

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Versions</CardTitle>
          <CardDescription>
            Select a version to inspect it above, or fork it directly.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {versions.length === 0 ? (
            <p className="text-sm text-muted-foreground">No published versions.</p>
          ) : (
            <ul className="space-y-2">
              {versions.map((version) => (
                <li
                  key={version.id}
                  className="flex items-center justify-between gap-3 rounded-md border border-border p-2"
                >
                  <button
                    type="button"
                    onClick={() => setShownVersion(version.version)}
                    className="min-w-0 flex-1 text-left"
                  >
                    <div className="flex items-center gap-2 text-sm">
                      <span className="font-medium">v{version.version}</span>
                      <Badge variant={version.status === "stable" ? "default" : "secondary"}>
                        {version.status}
                      </Badge>
                      {version.version === shown?.version && (
                        <span className="text-xs text-link">shown above</span>
                      )}
                      <span className="text-xs text-muted-foreground">
                        {formatDate(version.created_at)}
                      </span>
                    </div>
                    {version.changelog && (
                      <p className="mt-1 truncate text-xs text-muted-foreground">
                        {version.changelog}
                      </p>
                    )}
                  </button>
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy}
                    onClick={() => handleFork(version.version)}
                  >
                    <GitFork className="mr-1 h-3.5 w-3.5" />
                    Fork
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Reviews</CardTitle>
          <CardDescription>
            One per organization — saving again replaces your previous review.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {reviews.length === 0 ? (
            <p className="text-sm text-muted-foreground">No reviews yet.</p>
          ) : (
            <ul className="space-y-2">
              {reviews.map((review) => (
                <li key={review.id} className="rounded-md border border-border p-2">
                  <Stars value={review.rating} />
                  {review.comment && <p className="mt-1 text-sm">{review.comment}</p>}
                  <p className="mt-1 text-xs text-muted-foreground">
                    {formatDate(review.updated_at)}
                  </p>
                </li>
              ))}
            </ul>
          )}

          <div className="space-y-2 border-t border-border pt-4">
            <h4 className="text-sm font-medium">Your review</h4>
            <div className="flex items-center gap-2">
              <label htmlFor="review-rating" className="text-xs text-muted-foreground">
                Rating
              </label>
              <select
                id="review-rating"
                value={rating}
                onChange={(e) => setRating(Number(e.target.value))}
                className="h-9 rounded-md border border-input bg-card px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
              >
                {[5, 4, 3, 2, 1].map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </div>
            <textarea
              value={comment}
              onChange={(e) => setComment(e.target.value)}
              rows={3}
              placeholder="What did you use it for?"
              className="w-full rounded-md border border-input px-3 py-2 text-sm text-foreground shadow-sm placeholder:text-muted-foreground focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <div className="flex justify-end">
              <Button size="sm" disabled={busy} onClick={handleReview}>
                Save review
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

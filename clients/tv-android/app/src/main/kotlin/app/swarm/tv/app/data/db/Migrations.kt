/**
 * The versioned-script convention this app uses in place of a tool like
 * Python's yoyo (no Kotlin/Android equivalent of yoyo's directory-of-.sql-
 * files exists — Room's [androidx.room.migration.Migration] is the
 * idiomatic Android answer to the same problem: a numbered, ordered,
 * tested sequence of schema changes applied in order by the framework,
 * never edited once shipped):
 *
 * 1. Bump [app.swarm.tv.app.data.db.AppDatabase]'s `@Database(version = ...)`
 *    by exactly 1.
 * 2. Add one `Migration(oldVersion, newVersion) { db -> db.execSQL("...") }`
 *    entry to [MIGRATIONS] below, written as raw SQL (`ALTER TABLE`,
 *    `CREATE TABLE`, backfills, ...) — the migration script itself.
 * 3. Update the matching `@Entity`/`@Dao` Kotlin models in the same change
 *    so the compiled schema matches what the migration produces; a build
 *    with `ksp { arg("room.schemaLocation", ...) }` (see app/build.gradle.kts)
 *    fails if they drift apart.
 * 4. Never edit a [Migration] that has already shipped — like a yoyo
 *    migration file once applied, it's a historical record of what ran on
 *    real installs; a wrong step gets fixed by adding a new, later one.
 *
 * Empty today: version 1 is the schema's initial creation, which Room
 * derives directly from the `@Entity` annotations for a fresh install —
 * there is nothing to migrate *from* yet. The first real entry lands here
 * the day version 1 needs to change under installs that already exist.
 */
package app.swarm.tv.app.data.db

import androidx.room.migration.Migration

val MIGRATIONS: Array<Migration> = arrayOf()

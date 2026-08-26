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
 * Version 1 is the schema's initial creation, which Room derives directly
 * from the `@Entity` annotations for a fresh install — nothing to migrate
 * *from* there.
 */
package app.swarm.tv.app.data.db

import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase

/** Adds [KidModeSettingsEntity]'s table — a fresh install skips this (Room derives version 2's schema directly from the `@Entity` annotations), only an install upgrading from version 1 actually runs this SQL. */
private val MIGRATION_1_2 = object : Migration(1, 2) {
    override fun migrate(db: SupportSQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE IF NOT EXISTS kid_mode_settings (" +
                "id INTEGER NOT NULL PRIMARY KEY, " +
                "enabled INTEGER NOT NULL, " +
                "pin_hash TEXT NOT NULL, " +
                "pin_salt TEXT NOT NULL, " +
                "allowed_kinds TEXT NOT NULL, " +
                "allowed_genres TEXT, " +
                "max_movie_rating TEXT, " +
                "max_tv_rating TEXT" +
                ")",
        )
    }
}

/** Persists successful STUN-free LAN connections for dashboard restoration. */
private val MIGRATION_2_3 = object : Migration(2, 3) {
    override fun migrate(db: SupportSQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE IF NOT EXISTS local_server_connection (" +
                "cert_fingerprint TEXT NOT NULL PRIMARY KEY, " +
                "service_name TEXT NOT NULL, " +
                "name TEXT NOT NULL, " +
                "host TEXT NOT NULL, " +
                "peer_port INTEGER NOT NULL, " +
                "pairing_port INTEGER NOT NULL, " +
                "device_name TEXT NOT NULL, " +
                "last_connected_at INTEGER NOT NULL" +
                ")",
        )
        db.execSQL(
            "CREATE INDEX IF NOT EXISTS index_local_server_connection_last_connected_at " +
                "ON local_server_connection(last_connected_at)",
        )
    }
}

/** Adds the durable resolved-problem inbox and dismissal tombstones. */
private val MIGRATION_3_4 = object : Migration(3, 4) {
    override fun migrate(db: SupportSQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE IF NOT EXISTS client_notification (" +
                "`key` TEXT NOT NULL PRIMARY KEY, " +
                "server_id TEXT NOT NULL, " +
                "remote_id INTEGER NOT NULL, " +
                "server_name TEXT NOT NULL, " +
                "asset_title TEXT, " +
                "original_message TEXT NOT NULL, " +
                "comments TEXT, " +
                "resolved_at INTEGER NOT NULL, " +
                "dismissed INTEGER NOT NULL" +
                ")",
        )
        db.execSQL(
            "CREATE INDEX IF NOT EXISTS index_client_notification_resolved_at " +
                "ON client_notification(resolved_at)",
        )
    }
}

val MIGRATIONS: Array<Migration> = arrayOf(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4)

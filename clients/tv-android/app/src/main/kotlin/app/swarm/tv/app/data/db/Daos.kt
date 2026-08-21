package app.swarm.tv.app.data.db

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.Transaction
import kotlinx.coroutines.flow.Flow

@Dao
interface ServerConnectionDao {
    @Query("SELECT * FROM server_connection WHERE id = $SINGLETON_ROW_ID")
    suspend fun get(): ServerConnectionEntity?

    @Query("SELECT * FROM server_connection WHERE id = $SINGLETON_ROW_ID")
    fun observe(): Flow<ServerConnectionEntity?>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(entity: ServerConnectionEntity)

    /** Renames the saved connection's device-name column in place — used by the config page's edit field. No-op if nothing is saved yet. */
    @Query("UPDATE server_connection SET device_name = :name WHERE id = $SINGLETON_ROW_ID")
    suspend fun renameDevice(name: String)

    @Query("DELETE FROM server_connection")
    suspend fun clear()
}

@Dao
interface SwarmDao {
    @Query("SELECT * FROM swarm WHERE connection_id = $SINGLETON_ROW_ID ORDER BY name")
    suspend fun list(): List<SwarmEntity>

    @Query("SELECT * FROM swarm WHERE connection_id = $SINGLETON_ROW_ID ORDER BY name")
    fun observe(): Flow<List<SwarmEntity>>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsertAll(swarms: List<SwarmEntity>)

    @Query("DELETE FROM swarm WHERE id = :swarmId")
    suspend fun delete(swarmId: String)

    /** Replaces the whole joined-swarms set for the one saved connection — used after a join/leave/switch round trip to the STUN server, whose response is always the full authoritative list. */
    @Transaction
    suspend fun replaceAll(swarms: List<SwarmEntity>) {
        clearAll()
        upsertAll(swarms)
    }

    @Query("DELETE FROM swarm WHERE connection_id = $SINGLETON_ROW_ID")
    suspend fun clearAll()

    /**
     * Marks exactly [swarmId] active and every other row inactive in one
     * statement — see [SwarmEntity]'s doc comment for why this must never
     * be split into a separate clear + set. Passing null clears every row
     * (the "left every swarm" state) — written as an explicit `CASE`, not
     * a bare `id = :swarmId` comparison: SQL's three-valued logic makes
     * `id = NULL` evaluate to `NULL`, not `false`, which would try to
     * write `NULL` into this `NOT NULL` column and fail the whole update.
     */
    @Query(
        "UPDATE swarm SET is_active = CASE WHEN :swarmId IS NOT NULL AND id = :swarmId THEN 1 ELSE 0 END " +
            "WHERE connection_id = $SINGLETON_ROW_ID",
    )
    suspend fun setActive(swarmId: String?)
}

@Dao
interface AppSettingsDao {
    @Query("SELECT * FROM app_settings WHERE id = $SINGLETON_ROW_ID")
    suspend fun get(): AppSettingsEntity?

    @Query("SELECT * FROM app_settings WHERE id = $SINGLETON_ROW_ID")
    fun observe(): Flow<AppSettingsEntity?>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(entity: AppSettingsEntity)
}

@Dao
interface KidModeDao {
    @Query("SELECT * FROM kid_mode_settings WHERE id = $SINGLETON_ROW_ID")
    suspend fun get(): KidModeSettingsEntity?

    @Query("SELECT * FROM kid_mode_settings WHERE id = $SINGLETON_ROW_ID")
    fun observe(): Flow<KidModeSettingsEntity?>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(entity: KidModeSettingsEntity)

    @Query("DELETE FROM kid_mode_settings")
    suspend fun clear()
}

@Dao
interface LocalServerConnectionDao {
    @Query("SELECT * FROM local_server_connection ORDER BY last_connected_at DESC LIMIT 1")
    suspend fun mostRecent(): LocalServerConnectionEntity?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(entity: LocalServerConnectionEntity)

    @Query("DELETE FROM local_server_connection WHERE cert_fingerprint = :fingerprint")
    suspend fun delete(fingerprint: String)
}

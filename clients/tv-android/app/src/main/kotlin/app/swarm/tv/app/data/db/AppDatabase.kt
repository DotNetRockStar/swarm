package app.swarm.tv.app.data.db

import android.content.Context
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase

@Database(
    entities = [ServerConnectionEntity::class, SwarmEntity::class, AppSettingsEntity::class, KidModeSettingsEntity::class],
    version = 2,
    exportSchema = true,
)
abstract class AppDatabase : RoomDatabase() {
    abstract fun serverConnectionDao(): ServerConnectionDao
    abstract fun swarmDao(): SwarmDao
    abstract fun appSettingsDao(): AppSettingsDao
    abstract fun kidModeDao(): KidModeDao

    companion object {
        @Volatile private var instance: AppDatabase? = null

        fun getInstance(context: Context): AppDatabase =
            instance ?: synchronized(this) {
                instance ?: Room.databaseBuilder(context.applicationContext, AppDatabase::class.java, "swarm.db")
                    .addMigrations(*MIGRATIONS)
                    // Real concurrent access here: DAOs are read from
                    // Compose recompositions and written from ViewModel
                    // coroutines at the same time (e.g. the config screen
                    // observing server_connection while a join/leave call
                    // updates swarm rows) — WAL lets those overlap instead
                    // of serializing behind a single writer lock, the
                    // standard "optimized for a mobile app's access
                    // pattern" choice Room documents for exactly this case.
                    .setJournalMode(JournalMode.WRITE_AHEAD_LOGGING)
                    .build()
                    .also { instance = it }
            }
    }
}
